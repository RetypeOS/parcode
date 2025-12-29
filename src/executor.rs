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
    graph: &'graph TaskGraph<'a>,
    writer: &'graph SeqWriter<W>,
    registry: &'graph CompressorRegistry,
    use_compression: bool,
    abort_flag: AtomicBool,
    error_capture: Mutex<Option<ParcodeError>>,
    root_result: Mutex<Option<ChildRef>>,
}

#[cfg(feature = "parallel")]
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
    {
        execute_graph_parallel(graph, writer, registry, use_compression)
    }

    #[cfg(any(not(feature = "parallel"), target_arch = "wasm32"))]
    {
        execute_graph_serial_fallback(graph, writer, registry, use_compression)
    }
}
// ------------

/// PLACEHOLDER
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

    // 2. Identify initial leaves.
    use std::sync::atomic::Ordering;
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

/// PLACEHOLDER
pub fn execute_graph_sync<'a, W: Write + Send>(
    graph: &TaskGraph<'a>,
    writer: &SeqWriter<W>,
    registry: &CompressorRegistry,
    use_compression: bool,
) -> Result<ChildRef> {
    // Buffer re-usable for compression.
    let mut shared_buffer = Vec::with_capacity(128 * 1024);
    let mut root_result: Option<ChildRef> = None;

    // Iter nodes in reverse (ID descending)
    for node in graph.nodes().iter().rev() {
        // 1. Collect children results
        let completed_children_raw = {
            let mut guard = node
                .completed_children
                .lock()
                .map_err(|_| ParcodeError::Internal("Node mutex poisoned".into()))?;
            std::mem::take(&mut *guard)
        };

        let children_refs: Vec<ChildRef> = completed_children_raw
            .into_iter()
            .map(|opt| opt.ok_or_else(|| ParcodeError::Internal("Missing child result".into())))
            .collect::<Result<_>>()?;

        let is_chunkable = !children_refs.is_empty();

        // 2. Execute Job (Serialization)
        let raw_payload = node.job.execute(&children_refs)?;

        // 3. Compress & Format
        shared_buffer.clear();
        let config = node.job.config();

        // Default to LZ4 if compression enabled and config says 0 (None/Default)
        // config.compression_id == 0 means use default.
        let compression_id = if use_compression && config.compression_id == 0 {
            1 // LZ4
        } else {
            config.compression_id
        };

        // Fallback to None (ID 0) if requested algorithm unavailable
        let compressor = registry
            .get(compression_id)
            .unwrap_or_else(|_| registry.get(0).unwrap());

        let footer_size = if is_chunkable {
            (children_refs.len() * ChildRef::SIZE) + 4
        } else {
            0
        };

        shared_buffer.reserve(raw_payload.len() + footer_size + 1);

        // Compress directly to buffer
        compressor.compress_append(&raw_payload, &mut shared_buffer)?;

        // 4. Footer & Meta
        if is_chunkable {
            for child in &children_refs {
                shared_buffer.extend_from_slice(&child.to_bytes());
            }
            let count = u32::try_from(children_refs.len()).unwrap_or(u32::MAX);
            shared_buffer.extend_from_slice(&count.to_le_bytes());
        }

        let meta = MetaByte::new(is_chunkable, compressor.id());
        shared_buffer.push(meta.as_u8());

        // 5. Write to SeqWriter
        let offset = writer.write_all(&shared_buffer)?;

        let my_ref = ChildRef {
            offset,
            length: shared_buffer.len() as u64,
        };

        // 6. Propagate result to parent
        if let Some(parent_id) = node.parent {
            let parent_node = graph.get_node(parent_id);
            let slot = node.parent_slot.expect("Parent slot missing");
            parent_node.register_child_result(slot, my_ref)?;
        } else {
            root_result = Some(my_ref);
        }
    }

    // Flush writer buffers
    writer.flush()?;

    root_result
        .ok_or_else(|| ParcodeError::Internal("Graph execution completed but no root found".into()))
}

/// The worker function executed by Rayon threads.
/// It handles Serialization -> Compression -> Writing -> Notification.
#[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
fn process_node<'scope, 'a, W: Write + Send>(
    scope: &rayon::Scope<'scope>,
    ctx: &'scope ExecutionContext<'a, 'scope, W>,
    node: &'scope Node<'a>,
) {
    // 0. Fast abort check
    if ctx.should_abort() {
        return;
    }

    // 1. Prepare data

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

    let is_chunkable = !children_refs.is_empty();

    let raw_payload = match node.job.execute(&children_refs) {
        Ok(bytes) => bytes,
        Err(e) => {
            ctx.signal_error(e);
            return;
        }
    };

    // 2&3. Compression and formatting

    let config = node.job.config();
    let compression_id = if ctx.use_compression && config.compression_id == 0 {
        1
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

    let footer_size = if is_chunkable {
        (children_refs.len() * ChildRef::SIZE) + 4
    } else {
        0
    };

    let estimated_capacity = raw_payload.len() + footer_size + 1;
    let mut final_buffer = Vec::with_capacity(estimated_capacity);

    if let Err(e) = compressor.compress_append(&raw_payload, &mut final_buffer) {
        ctx.signal_error(e);
        return;
    }

    if is_chunkable {
        for child in &children_refs {
            final_buffer.extend_from_slice(&child.to_bytes());
        }
        let count = u32::try_from(children_refs.len()).unwrap_or(u32::MAX);
        final_buffer.extend_from_slice(&count.to_le_bytes());
    }

    let meta = MetaByte::new(is_chunkable, compressor.id());
    final_buffer.push(meta.as_u8());

    // 4. Writting

    let write_result = ctx.writer.write_all(&final_buffer);
    let offset = match write_result {
        Ok(off) => off,
        Err(e) => {
            ctx.signal_error(e);
            return;
        }
    };

    let my_ref = ChildRef {
        offset,
        length: final_buffer.len() as u64,
    };

    // 5. Propagate

    if let Some(parent_id) = node.parent {
        let parent_node = ctx.graph.get_node(parent_id);

        let slot = node.parent_slot.expect("Parent set but slot missing");
        if let Err(e) = parent_node.register_child_result(slot, my_ref) {
            ctx.signal_error(e);
            return;
        }

        let prev_deps = parent_node.atomic_deps.fetch_sub(1, Ordering::SeqCst);

        if prev_deps == 1 {
            scope.spawn(move |s| process_node(s, ctx, parent_node));
        }
    } else {
        ctx.capture_root_result(my_ref);
    }
}
