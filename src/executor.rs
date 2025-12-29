//! The High-Performance Parallel Executor.
//!
//! This module orchestrates the "Bottom-Up" execution strategy. It is designed
//! to be purely reactive: completed children trigger the scheduling of their parents,
//! eliminating the need for a central polling loop and reducing latency.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::compression::CompressorRegistry;
use crate::error::{ParcodeError, Result};
use crate::format::{ChildRef, MetaByte};
use crate::graph::{Node, TaskGraph};
use crate::io::SeqWriter;
use std::io::Write;

/// Context shared among all worker threads.
struct ExecutionContext<'a, 'graph, W: Write + Send> {
    graph: &'graph TaskGraph<'a>, // The graph holds data living for 'a
    writer: &'graph SeqWriter<W>,
    registry: &'graph CompressorRegistry,
    use_compression: bool,
    abort_flag: AtomicBool,
    error_capture: Mutex<Option<ParcodeError>>,
    root_result: Mutex<Option<ChildRef>>,
}

impl<'a, 'graph, W: Write + Send> ExecutionContext<'a, 'graph, W> {
    fn signal_error(&self, err: ParcodeError) {
        let mut guard = self.error_capture.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            *guard = Some(err);
            self.abort_flag.store(true, Ordering::SeqCst);
        }
    }

    fn should_abort(&self) -> bool {
        self.abort_flag.load(Ordering::Relaxed)
    }

    fn capture_root_result(&self, result: ChildRef) {
        let mut guard = self.root_result.lock().unwrap_or_else(|p| p.into_inner());
        *guard = Some(result);
    }
}

/// Entry point for the execution engine.
///
/// This function consumes the graph (conceptually) and drives the I/O writing process.
/// It returns the `ChildRef` of the Root Chunk, which is needed to write the Global Header.
///
/// # Type Parameters
///
/// * `W`: The writer type. Must implement `std::io::Write` and `Send`.
pub fn execute_graph<'a, W: Write + Send>(
    graph: &TaskGraph<'a>,
    writer: &SeqWriter<W>,
    registry: &CompressorRegistry,
    use_compression: bool,
) -> Result<ChildRef> {
    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    return execute_graph_parallel(graph, writer, registry, use_compression);

    #[cfg(not(feature = "parallel"))]
    return execute_graph_serial_fallback(graph, writer, registry, use_compression);
}
// ------------
#[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
fn execute_graph_parallel<'a, W: Write + Send>(
    graph: &TaskGraph<'a>,
    writer: &SeqWriter<W>,
    registry: &CompressorRegistry,
    use_compression: bool,
) -> Result<ChildRef> {
    // 1. Setup the shared context.
    let ctx = ExecutionContext {
        graph,
        writer,
        registry,
        use_compression,
        abort_flag: AtomicBool::new(false),
        error_capture: Mutex::new(None),
        root_result: Mutex::new(None),
    };

    // 2. Identify initial leaves (Nodes with 0 dependencies).
    // These are the spark plugs that start the engine.
    let leaves: Vec<&Node<'a>> = graph
        .nodes()
        .iter()
        .filter(|n| n.atomic_deps.load(Ordering::SeqCst) == 0)
        .collect();

    if leaves.is_empty() && !graph.is_empty() {
        return Err(ParcodeError::Internal(
            "Graph has nodes but no leaves. Cyclic dependency detected.".into(),
        ));
    }

    // 3. Launch the Rayon Scope.
    // The scope ensures all spawned threads complete before this block exits.
    rayon::scope(|s| {
        let ctx_ref = &ctx;
        for leaf in leaves {
            s.spawn(move |s| process_node(s, ctx_ref, leaf));
        }
    });

    // 4. Check for errors after the scope joins.
    if ctx.should_abort() {
        let guard = ctx.error_capture.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(err) = guard.as_ref() {
            return Err(err.clone());
        }
        return Err(ParcodeError::Internal("Unknown execution error".into()));
    }

    // 5. Return the Root Result.
    let root_guard = ctx
        .root_result
        .lock()
        .map_err(|_| ParcodeError::Internal("Root result mutex poisoned".into()))?;

    (*root_guard).ok_or_else(|| ParcodeError::Internal("Graph execution incomplete".into()))
}

#[cfg(not(feature = "parallel"))]
fn execute_graph_serial_fallback<'a, W: Write + Send>(
    graph: &TaskGraph<'a>,
    writer: &SeqWriter<W>,
    registry: &CompressorRegistry,
    use_compression: bool,
) -> Result<ChildRef> {
    let mut shared_buffer = Vec::with_capacity(128 * 1024);
    let mut root_result: Option<ChildRef> = None;

    // TODO: maybe i will refactor this
    for node in graph.nodes().iter().rev() {
        // 1. Collect children results
        let completed_children_raw = {
            let mut guard = node.completed_children.lock().expect("Mutex poisoned");
            std::mem::take(&mut *guard)
        };

        let children_refs: Vec<ChildRef> = completed_children_raw
            .into_iter()
            .map(|opt| {
                opt.ok_or_else(|| crate::ParcodeError::Internal("Missing child result".into()))
            })
            .collect::<Result<_>>()?;

        let is_chunkable = !children_refs.is_empty();

        // 2. Execute Job
        let raw_payload = node.job.execute(&children_refs)?;

        // 3. Compress
        shared_buffer.clear();
        let config = node.job.config();
        let compression_id = if use_compression && config.compression_id == 0 {
            1
        } else {
            config.compression_id
        };
        let compressor = registry.get(compression_id)?;

        let footer_size = if is_chunkable {
            (children_refs.len() * ChildRef::SIZE) + 4
        } else {
            0
        };
        shared_buffer.reserve(raw_payload.len() + footer_size + 1);
        compressor.compress_append(&raw_payload, &mut shared_buffer)?;

        // 4. Footer & Meta
        if is_chunkable {
            for child in &children_refs {
                shared_buffer.extend_from_slice(&child.to_bytes());
            }
            let count = u32::try_from(children_refs.len()).unwrap_or(u32::MAX);
            shared_buffer.extend_from_slice(&count.to_le_bytes());
        }
        let meta = crate::format::MetaByte::new(is_chunkable, compressor.id());
        shared_buffer.push(meta.as_u8());

        // 5. Write to SeqWriter
        let offset = writer.write_all(&shared_buffer)?;

        let my_ref = ChildRef {
            offset,
            length: shared_buffer.len() as u64,
        };

        // 6. Propagate to parent
        if let Some(parent_id) = node.parent {
            let parent_node = graph.get_node(parent_id);
            let slot = node.parent_slot.expect("Parent slot missing");
            parent_node.register_child_result(slot, my_ref)?;
        } else {
            root_result = Some(my_ref);
        }
    }

    writer.flush()?;

    root_result.ok_or_else(|| crate::ParcodeError::Internal("Graph execution incomplete".into()))
}

/// Executes the graph synchronously with aggressive optimizations.
///
/// # Optimizations
/// 1. **Implicit Topology:** Iterates nodes in reverse order (ID descending).
///    Since `ParcodeVisitor` builds the tree recursively (Parent creates self, then visits children),
///    children always have higher IDs than parents. Reverse iteration guarantees dependencies are met.
///    *Benefit:* Eliminates O(N) allocations for dependency tracking and queues.
///
/// 2. **Lockless I/O:** Takes ownership of the underlying writer to bypass Mutex overhead.
///
/// 3. **Buffer Reuse:** Uses a single, growing buffer for all compression ops.
///
/// # Type Parameters
///
/// * `W`: The writer type. Must implement `std::io::Write` and `Send`.
pub fn execute_graph_sync<'a, W: Write + Send>(
    graph: &TaskGraph<'a>,
    writer: SeqWriter<W>,
    registry: &CompressorRegistry,
    use_compression: bool,
) -> Result<ChildRef> {
    // 1. Bypass the Mutex lock. We own the writer now.
    let mut raw_writer = writer.into_inner()?;
    // TODO: we revise this logic in another time.
    // At the moment, i assume start offset is 0 for now for simplicity in generic context, or we could add a `base_offset` param.
    let mut current_offset = 0u64;

    // 2. Reusable buffer. Start with 128KB to minimize initial reallocs.
    let mut shared_buffer = Vec::with_capacity(128 * 1024);
    let mut root_result: Option<ChildRef> = None;

    // 3. Execution Loop: Reverse Iteration (Zero-Alloc Topology)
    // The nodes are stored in a Vec. Index N is the Child, Index 0 is Root.
    // Iterating N -> 0 ensures children are processed before parents.
    let nodes = graph.nodes();
    for node in nodes.iter().rev() {
        // --- STEP A: Gather Children Results ---
        // Even though we are sync, the Node struct uses Mutex for safety.
        // In this loop, contention is impossible.
        let completed_children_raw = {
            let mut guard = node
                .completed_children
                .lock()
                .map_err(|_| ParcodeError::Internal("Node mutex poisoned".into()))?;
            // TAKE the value to free memory in the node immediately
            std::mem::take(&mut *guard)
        };

        // Efficiently unwrap without extra allocations if possible
        // We can iterate the vector directly.
        let is_chunkable = !completed_children_raw.is_empty();

        // --- STEP B: Serialize (User logic) ---
        // Map Option<ChildRef> -> ChildRef
        // We reconstruct the vector only because execute() signature requires slice.
        // In a deeper refactor, we would change execute() to take an iterator to avoid this alloc.
        let children_refs: Vec<ChildRef> = completed_children_raw
            .into_iter()
            .map(|opt| opt.ok_or_else(|| ParcodeError::Internal("Missing child result".into())))
            .collect::<Result<_>>()?;

        let raw_payload = node.job.execute(&children_refs)?;

        // --- STEP C: Compress & Format ---
        shared_buffer.clear();

        let config = node.job.config();
        let compression_id = if use_compression && config.compression_id == 0 {
            1 // Default to LZ4
        } else {
            config.compression_id
        };
        let compressor = registry.get(compression_id)?;

        let footer_size = if is_chunkable {
            (children_refs.len() * ChildRef::SIZE) + 4
        } else {
            0
        };

        // Heuristic reserve
        shared_buffer.reserve(raw_payload.len() + footer_size + 1);

        // Compress
        compressor.compress_append(&raw_payload, &mut shared_buffer)?;

        // Footer
        if is_chunkable {
            for child in &children_refs {
                shared_buffer.extend_from_slice(&child.to_bytes());
            }
            let count = u32::try_from(children_refs.len()).unwrap_or(u32::MAX);
            shared_buffer.extend_from_slice(&count.to_le_bytes());
        }

        // MetaByte
        let meta = MetaByte::new(is_chunkable, compressor.id());
        shared_buffer.push(meta.as_u8());

        // --- STEP D: Write to Disk (Lockless) ---
        raw_writer.write_all(&shared_buffer)?;

        let chunk_len = shared_buffer.len() as u64;
        let my_ref = ChildRef {
            offset: current_offset,
            length: chunk_len,
        };
        current_offset += chunk_len;

        // --- STEP E: Propagate to Parent ---
        if let Some(parent_id) = node.parent {
            // Note: We are iterating strictly backwards. Parent ID is GUARANTEED
            // to be smaller than current node.id, so parent is "future" work.
            let parent_node = graph.get_node(parent_id);
            let slot = node.parent_slot.expect("Parent set but slot missing");

            // This is the only Mutex usage remaining (registering result).
            // It's just a memory write, extremely fast.
            parent_node.register_child_result(slot, my_ref)?;
        } else {
            // No parent? Must be root.
            root_result = Some(my_ref);
        }
    }

    // Flush is critical
    raw_writer.flush()?;

    root_result
        .ok_or_else(|| ParcodeError::Internal("Graph execution completed but no root found".into()))
}

/// The worker function executed by Rayon threads.
/// It handles Serialization -> Compression -> Writing -> Notification.
fn process_node<'scope, 'a, W: Write + Send>(
    scope: &rayon::Scope<'scope>,
    ctx: &'scope ExecutionContext<'a, 'scope, W>,
    node: &'scope Node<'a>,
) {
    // 0. Fast abort check
    if ctx.should_abort() {
        return;
    }

    // --- STEP 1: PREPARE DATA (CPU BOUND) ---

    let completed_children_raw = {
        let lock = node
            .completed_children
            .lock()
            .map_err(|_| ParcodeError::Internal("Node mutex poisoned".into()));
        match lock {
            Ok(mut guard) => std::mem::take(&mut *guard),
            Err(e) => {
                ctx.signal_error(e);
                return;
            }
        }
    };

    // Retrieve the results from children (if any).
    // We lock the mutex, take the vector out (swap with empty) to consume it efficiently.
    // Since we used slots, the order is already correct. We just need to unwrap the Options.
    let children_refs: Vec<ChildRef> = match completed_children_raw
        .into_iter()
        .map(|opt| opt.ok_or_else(|| ParcodeError::Internal("Missing child result".into())))
        .collect()
    {
        Ok(v) => v,
        Err(e) => {
            ctx.signal_error(e);
            return;
        }
    };

    // A node is "Chunkable" (Bit 0 set) if it had children dependencies.
    // Note: The optimizer pass (Phase 2) might have removed children and inlined them,
    // so we trust `children_refs`.
    let is_chunkable = !children_refs.is_empty();

    // Execute the job (Serialization).
    // This is where the user's `Vec<T>` turns into bytes.
    // We pass the children refs so they can be embedded in the footer/table if needed
    // by the specific implementation, or we append the standard footer below.
    let raw_payload = match node.job.execute(&children_refs) {
        Ok(bytes) => bytes,
        Err(e) => {
            ctx.signal_error(e);
            return;
        }
    };

    // --- STEP 2 & 3: COMPRESSION & FORMATTING (CPU BOUND) ---

    // 1. Get Job Config
    let config = node.job.config();

    // 2. Find Algorithm
    let compression_id = if ctx.use_compression && config.compression_id == 0 {
        1 // Default to LZ4
    } else {
        config.compression_id
    };

    let compressor = match ctx.registry.get(compression_id) {
        Ok(c) => c,
        Err(e) => {
            ctx.signal_error(e);
            return;
        }
    };

    // 3. Prepare Final Buffer
    // We need to calculate footer size to reserve space.
    let footer_size = if is_chunkable {
        (children_refs.len() * ChildRef::SIZE) + 4
    } else {
        0
    };

    // Heuristic: Allocate enough for raw payload + footer + meta.
    // If compression shrinks it, great. If it expands (rare), it will realloc.
    let estimated_capacity = raw_payload.len() + footer_size + 1;
    let mut final_buffer = Vec::with_capacity(estimated_capacity);

    // 4. Compress directly into final buffer
    if let Err(e) = compressor.compress_append(&raw_payload, &mut final_buffer) {
        ctx.signal_error(e);
        return;
    }

    // 5. Append Footer
    if is_chunkable {
        for child in &children_refs {
            final_buffer.extend_from_slice(&child.to_bytes());
        }
        let count = u32::try_from(children_refs.len()).unwrap_or(u32::MAX);
        final_buffer.extend_from_slice(&count.to_le_bytes());
    }

    // 6. Append MetaByte
    let meta = MetaByte::new(is_chunkable, compressor.id());
    final_buffer.push(meta.as_u8());

    // --- STEP 4: WRITING (I/O BOUND) ---
    // This is the only serialization point (Mutex).

    let write_result = ctx.writer.write_all(&final_buffer);
    let offset = match write_result {
        Ok(off) => off,
        Err(e) => {
            ctx.signal_error(e);
            return;
        }
    };

    // Create the Reference for this newly written chunk.
    let my_ref = ChildRef {
        offset,
        length: final_buffer.len() as u64,
    };

    // --- STEP 5: PROPAGATION ---

    if let Some(parent_id) = node.parent {
        let parent_node = ctx.graph.get_node(parent_id);

        // A. Register our result in the parent
        let slot = node.parent_slot.expect("Parent set but slot missing");
        if let Err(e) = parent_node.register_child_result(slot, my_ref) {
            ctx.signal_error(e);
            return;
        }

        // B. Decrement parent's dependency counter.
        // `fetch_sub` returns the PREVIOUS value.
        // If previous was 1, it means it is NOW 0 -> Ready to fire.
        let prev_deps = parent_node.atomic_deps.fetch_sub(1, Ordering::SeqCst);

        if prev_deps == 1 {
            // We are the last child! We have the honor of waking the parent.
            // Spawn into the existing scope.
            scope.spawn(move |s| process_node(s, ctx, parent_node));
        }
    } else {
        // We have no parent. We are the ROOT.
        ctx.capture_root_result(my_ref);
    }
}
