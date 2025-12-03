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

/// Context shared among all worker threads.
struct ExecutionContext<'a, 'graph> {
    graph: &'graph TaskGraph<'a>, // The graph holds data living for 'a
    writer: &'graph SeqWriter,
    registry: &'graph CompressorRegistry,
    abort_flag: AtomicBool,
    error_capture: Mutex<Option<ParcodeError>>,
    root_result: Mutex<Option<ChildRef>>,
}

impl<'a, 'graph> ExecutionContext<'a, 'graph> {
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
pub fn execute_graph<'a>(
    graph: &TaskGraph<'a>,
    writer: &SeqWriter,
    registry: &CompressorRegistry,
) -> Result<ChildRef> {
    // 1. Setup the shared context.
    let ctx = ExecutionContext {
        graph,
        writer,
        registry,
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

    if leaves.is_empty() && graph.len() > 0 {
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

    root_guard
        .clone()
        .ok_or_else(|| ParcodeError::Internal("Graph execution incomplete".into()))
}

/// The worker function executed by Rayon threads.
/// It handles Serialization -> Compression -> Writing -> Notification.
fn process_node<'scope, 'a>(
    scope: &rayon::Scope<'scope>,
    ctx: &'scope ExecutionContext<'a, 'scope>,
    node: &'scope Node<'a>,
) {
    // 0. Fast abort check
    if ctx.should_abort() {
        return;
    }

    // --- STEP 1: PREPARE DATA (CPU BOUND) ---

    let mut completed_children_raw = {
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

    completed_children_raw.sort_by_key(|(id, _)| *id);

    // Retrieve the results from children (if any).
    // We lock the mutex, take the vector out (swap with empty) to consume it efficiently.
    let children_refs: Vec<ChildRef> = completed_children_raw.into_iter().map(|(_, r)| r).collect();

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

    // --- STEP 2: COMPRESSION (CPU BOUND) ---

    // 1. Get Job Config
    let config = node.job.config();

    // 2. Find Algorithm
    let compressor = match ctx.registry.get(config.compression_id) {
        Ok(c) => c,
        Err(e) => {
            ctx.signal_error(e);
            return;
        }
    };

    // 3. Compress
    let compressed_payload = match compressor.compress(&raw_payload) {
        Ok(c) => c,
        Err(e) => {
            ctx.signal_error(e);
            return;
        }
    };

    // --- STEP 3: FORMATTING (CPU BOUND) ---

    // Construct the final binary block:
    // [Compressed Data] + [Footer Table (if chunkable)] + [MetaByte]

    // We need to calculate total size to allocate buffer once.
    let footer_size = if is_chunkable {
        (children_refs.len() * ChildRef::SIZE) + 4
    } else {
        0
    };
    let total_size = compressed_payload.len() + footer_size + 1;
    let mut final_buffer = Vec::with_capacity(total_size);
    final_buffer.extend_from_slice(&compressed_payload);

    if is_chunkable {
        for child in &children_refs {
            final_buffer.extend_from_slice(&child.to_bytes());
        }
        let count = children_refs.len() as u32;
        final_buffer.extend_from_slice(&count.to_le_bytes());
    }
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
        length: total_size as u64,
    };

    // --- STEP 5: PROPAGATION ---

    if let Some(parent_id) = node.parent {
        let parent_node = ctx.graph.get_node(parent_id);

        // A. Register our result in the parent
        if let Err(e) = parent_node.register_child_result(node.id, my_ref) {
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
