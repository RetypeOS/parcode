// src/visitor_impls.rs

//! Implementation of `ParcodeVisitor` for standard Rust collections.
//!
//! # Sharding Strategy V3: Concurrency-Aware & Adaptive
//!
//! This module decides how to split a `Vec<T>` into shards. The decision is based on:
//! 1. **Byte Size:** We target ~128KB per chunk to optimize I/O and compression.
//! 2. **CPU Saturation:** We ensure we create enough chunks to feed all available CPU cores,
//!    even if that means creating chunks smaller than 128KB (down to a 4KB floor).
//! 3. **Real Data Sampling:** We measure actual serialization size to handle Heap data.

use crate::error::{ParcodeError, Result};
use crate::format::ChildRef;
use crate::graph::{ChunkId, SerializationJob, TaskGraph};
use crate::visitor::ParcodeVisitor;
use serde::{Deserialize, Serialize};

// --- TUNING CONSTANTS ---

/// The ideal size for a chunk on disk. Optimized for SSD throughput.
const TARGET_SHARD_SIZE_BYTES: u64 = 128 * 1024; // 128 KB

/// The absolute minimum size for a chunk to avoid OS overhead hell.
const MIN_SHARD_SIZE_BYTES: u64 = 4 * 1024; // 4 KB

/// The multiplier for CPU cores. If we have 8 cores, we want at least 8 * 4 = 32 chunks
/// to allow for effective work-stealing and load balancing.
const TASKS_PER_CORE: usize = 4;

// --- Internal Metadata Structures ---

#[derive(Clone, Serialize, Deserialize, Debug)]
struct ShardRun {
    item_count: u32,
    repeat: u32,
}

#[derive(Clone)]
struct VecContainerJob {
    shard_runs: Vec<ShardRun>,
    total_items: u64,
}

impl SerializationJob for VecContainerJob {
    fn execute(&self, _children_refs: &[ChildRef]) -> Result<Vec<u8>> {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&self.total_items.to_le_bytes());
        let runs_bytes =
            bincode::serde::encode_to_vec(&self.shard_runs, bincode::config::standard())
                .map_err(|e| ParcodeError::Serialization(e.to_string()))?;
        buffer.extend_from_slice(&runs_bytes);
        Ok(buffer)
    }
    fn estimated_size(&self) -> usize {
        8 + (self.shard_runs.len() * 8)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Clone)]
struct VecShardJob<T> {
    data: Vec<T>,
}

impl<T> SerializationJob for VecShardJob<T>
where
    T: Serialize + Send + Sync + 'static,
{
    fn execute(&self, _children_refs: &[ChildRef]) -> Result<Vec<u8>> {
        bincode::serde::encode_to_vec(&self.data, bincode::config::standard())
            .map_err(|e| ParcodeError::Serialization(e.to_string()))
    }
    fn estimated_size(&self) -> usize {
        self.data.len() * std::mem::size_of::<T>()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// --- Visitor Implementation ---

impl<T> ParcodeVisitor for Vec<T>
where
    T: ParcodeVisitor + Clone + Send + Sync + 'static + Serialize,
{
    fn visit(&self, graph: &mut TaskGraph, parent_id: Option<ChunkId>) {
        let total_len = self.len();
        let items_per_shard;

        if total_len == 0 {
            items_per_shard = 1;
        } else {
            // --- STEP 1: MEASURE DATA COST ---

            // Sample up to 8 items to get an average size
            let sample_count = total_len.min(8);
            let sample_slice = &self[0..sample_count];

            let sample_size_bytes =
                match bincode::serde::encode_to_vec(sample_slice, bincode::config::standard()) {
                    Ok(vec) => vec.len() as u64,
                    Err(_) => 0,
                };

            // Calculate Average Bytes Per Item (At least 1 byte)
            let avg_item_size = if sample_size_bytes > 0 {
                (sample_size_bytes / sample_count as u64).max(1)
            } else {
                (std::mem::size_of::<T>() as u64).max(1)
            };

            // --- STEP 2: CALCULATE STRATEGIES ---

            // Strategy A: I/O Optimized (Fill 128KB chunks)
            let count_by_io = (TARGET_SHARD_SIZE_BYTES / avg_item_size).max(1) as usize;

            // Strategy B: CPU Optimized (Fill cores)
            // We want at least (Cores * 4) chunks to keep Rayon busy.
            let num_cpus = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1);
            let target_parallel_chunks = num_cpus * TASKS_PER_CORE;
            let count_by_cpu = (total_len / target_parallel_chunks).max(1);

            // --- STEP 3: FUSE STRATEGIES ---

            // We prefer the smaller count (more chunks) to favor parallelism,
            // UNLESS it creates chunks that are too small (< 4KB).

            let candidate_count = count_by_io.min(count_by_cpu);

            // Verify the physical size of this candidate
            let estimated_chunk_size = candidate_count as u64 * avg_item_size;

            if estimated_chunk_size < MIN_SHARD_SIZE_BYTES {
                // Too small! Scale up to hit the minimum byte floor.
                // This prevents creating 1000 chunks of 10 bytes.
                items_per_shard = (MIN_SHARD_SIZE_BYTES / avg_item_size).max(1) as usize;
            } else {
                items_per_shard = candidate_count;
            }
        }

        // -----------------------------------

        let chunks: Vec<&[T]> = self.chunks(items_per_shard).collect();

        // Build RLE Metadata
        let mut shard_runs: Vec<ShardRun> = Vec::new();
        if !chunks.is_empty() {
            let mut current_run = ShardRun {
                item_count: chunks[0].len() as u32,
                repeat: 0,
            };

            for chunk in &chunks {
                let len = chunk.len() as u32;
                if len == current_run.item_count {
                    current_run.repeat += 1;
                } else {
                    shard_runs.push(current_run);
                    current_run = ShardRun {
                        item_count: len,
                        repeat: 1,
                    };
                }
            }
            shard_runs.push(current_run);
        }

        // Register Container Node
        let container_job = Box::new(VecContainerJob {
            shard_runs,
            total_items: self.len() as u64,
        });
        let my_id = graph.add_node(container_job);

        if let Some(pid) = parent_id {
            graph.link_parent_child(pid, my_id);
        }

        if self.is_empty() {
            return;
        }

        // Create Shard Nodes
        for chunk_slice in chunks {
            let shard_data = chunk_slice.to_vec();
            let shard_job = Box::new(VecShardJob { data: shard_data });

            let shard_id = graph.add_node(shard_job);
            graph.link_parent_child(my_id, shard_id);

            for item in chunk_slice {
                item.visit(graph, Some(shard_id));
            }
        }
    }

    fn create_job(&self) -> Box<dyn SerializationJob> {
        Box::new(VecContainerJob {
            shard_runs: Vec::new(),
            total_items: 0,
        })
    }
}

#[derive(Clone)]
struct PrimitiveJob<T>(T);

impl<T> SerializationJob for PrimitiveJob<T>
where
    T: Serialize + Send + Sync + Clone + 'static,
{
    fn execute(&self, _: &[ChildRef]) -> Result<Vec<u8>> {
        // Serialización simple sin hijos
        bincode::serde::encode_to_vec(&self.0, bincode::config::standard())
            .map_err(|e| ParcodeError::Serialization(e.to_string()))
    }

    fn estimated_size(&self) -> usize {
        std::mem::size_of::<T>()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

macro_rules! impl_primitive_visitor {
    ($($t:ty),*) => {
        $(
            impl ParcodeVisitor for $t {
                fn visit(&self, graph: &mut TaskGraph, parent_id: Option<ChunkId>) {
                    // OPTIMIZACIÓN CRÍTICA:
                    // Si tenemos un padre (ej: estamos dentro de un Vec<u64>), NO creamos un nodo.
                    // Somos datos "inlined" dentro del payload del padre.
                    // Solo creamos nodo si somos la RAÍZ absoluta (ej: Parcode::save("num.par", &10u64)).
                    if parent_id.is_none() {
                        let job = self.create_job();
                        graph.add_node(job);
                    }
                }

                fn create_job(&self) -> Box<dyn SerializationJob> {
                    Box::new(PrimitiveJob(self.clone()))
                }
            }
        )*
    }
}

// Aplicar a todos los tipos estándar
impl_primitive_visitor!(
    u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, f32, f64, bool, String
);
