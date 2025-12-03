//! Implementation of `ParcodeVisitor` for standard Rust collections.
//!
//! # Sharding Strategy V3: Adaptive and Concurrency-Aware
//!
//! This module decides how to split a `Vec<T>` into shards. The decision is based on:
//! 1.  **Size in Bytes:** We aim for ~128KB per chunk to optimize SSD throughput and compression.
//! 2.  **CPU Saturation:** We ensure enough chunks are created to feed all available cores,
//!     even if it means creating smaller chunks (down to a floor of 4KB).
//! 3.  **Real Data Sampling:** We measure the actual serialization size of a sample to handle
//!     heap-allocated data (like `Vec<String>`) accurately.

use crate::error::{ParcodeError, Result};
use crate::format::ChildRef;
use crate::graph::{ChunkId, JobConfig, SerializationJob, TaskGraph};
use crate::visitor::ParcodeVisitor;
use serde::{Deserialize, Serialize};

// --- TUNING CONSTANTS ---

/// Ideal size for a chunk on disk. Optimized for SSD throughput.
const TARGET_SHARD_SIZE_BYTES: u64 = 128 * 1024; // 128 KB

/// Absolute minimum size to avoid excessive OS/graph overhead.
const MIN_SHARD_SIZE_BYTES: u64 = 4 * 1024; // 4 KB

/// CPU core multiplier. If we have 8 cores, we want at least 32 tasks
/// to allow effective "work-stealing" and load balancing in Rayon.
const TASKS_PER_CORE: usize = 4;

// --- Internal Metadata Structures ---

/// RLE (Run-Length Encoding) structure to map logical indices to physical shards.
#[derive(Clone, Serialize, Deserialize, Debug)]
struct ShardRun {
    item_count: u32,
    repeat: u32,
}

/// The job that serializes the Vector Container Node.
/// Contains only metadata (RLE table and total length), not the data itself.
#[derive(Clone)]
struct VecContainerJob {
    shard_runs: Vec<ShardRun>,
    total_items: u64,
}

// ContainerJob owns its metadata, so it works for any lifetime 'a
impl<'a> SerializationJob<'a> for VecContainerJob {
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
}

/// The job that serializes a real data Shard.
/// Contains a subset of the original vector (`data`).
#[derive(Clone)]
struct VecShardJob<'a, T> {
    data: &'a [T],
}

impl<'a, T> SerializationJob<'a> for VecShardJob<'a, T>
where
    T: Serialize + Send + Sync + 'static,
{
    fn execute(&self, _children_refs: &[ChildRef]) -> Result<Vec<u8>> {
        // Serialize the SLICE directly. No cloning occurred during graph build.
        bincode::serde::encode_to_vec(self.data, bincode::config::standard())
            .map_err(|e| ParcodeError::Serialization(e.to_string()))
    }

    fn estimated_size(&self) -> usize {
        self.data.len() * std::mem::size_of::<T>()
    }
}

// --- Visitor Implementation ---

impl<T> ParcodeVisitor for Vec<T>
where
    T: ParcodeVisitor + Clone + Send + Sync + 'static + Serialize,
{
    fn visit<'a>(
        &'a self,
        graph: &mut TaskGraph<'a>,
        parent_id: Option<ChunkId>,
        config_override: Option<JobConfig>,
    ) {
        let total_len = self.len();
        let items_per_shard;

        // --- PHASE 1: SHARDING STRATEGY CALCULATION ---
        if total_len == 0 {
            items_per_shard = 1;
        } else {
            // 1. Measure data cost (Sampling)
            // We take up to 8 elements to estimate the real size (useful for Strings/Heap).
            let sample_count = total_len.min(8);
            let sample_slice = &self[0..sample_count];

            let sample_size_bytes =
                match bincode::serde::encode_to_vec(sample_slice, bincode::config::standard()) {
                    Ok(vec) => vec.len() as u64,
                    Err(_) => 0,
                };

            // Calculate average bytes per item (minimum 1 byte to avoid div by zero)
            let avg_item_size = if sample_size_bytes > 0 {
                (sample_size_bytes / sample_count as u64).max(1)
            } else {
                (std::mem::size_of::<T>() as u64).max(1)
            };

            // 2. Calculate Strategies

            // Strategy A: Optimized for I/O (Fill 128KB chunks)
            let count_by_io = (TARGET_SHARD_SIZE_BYTES / avg_item_size).max(1) as usize;

            // Strategy B: Optimized for CPU (Fill cores)
            // We want enough tasks to keep Rayon busy.
            let num_cpus = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1);
            let target_parallel_chunks = num_cpus * TASKS_PER_CORE;
            let count_by_cpu = (total_len / target_parallel_chunks).max(1);

            // 3. Strategy Fusion
            // We prefer more chunks (CPU) unless they are ridiculously small.
            let candidate_count = count_by_io.min(count_by_cpu);

            // Verify physical size of the candidate
            let estimated_chunk_size = candidate_count as u64 * avg_item_size;

            if estimated_chunk_size < MIN_SHARD_SIZE_BYTES {
                // Too small. Scale to meet the 4KB minimum.
                items_per_shard = (MIN_SHARD_SIZE_BYTES / avg_item_size).max(1) as usize;
            } else {
                items_per_shard = candidate_count;
            }
        }

        // --- PHASE 2: GRAPH CONSTRUCTION ---

        // Generate slices (views) of the data without copying memory yet.
        let chunks: Vec<&[T]> = self.chunks(items_per_shard).collect();

        // Build RLE metadata
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

        // 1. Create and Register the Container Node
        let container_inner = Box::new(VecContainerJob {
            shard_runs,
            total_items: self.len() as u64,
        });

        // Apply configuration (override) to the container if it exists.
        // If the user requests LZ4, the container is also marked as LZ4 (even if small).
        let container_job: Box<dyn SerializationJob<'a> + 'a> = if let Some(cfg) = config_override {
            Box::new(crate::rt::ConfiguredJob::new(container_inner, cfg))
        } else {
            container_inner
        };

        let my_id = graph.add_node(container_job);
        if let Some(pid) = parent_id {
            graph.link_parent_child(pid, my_id);
        }

        if self.is_empty() {
            return;
        }

        // 2. Create Shard Nodes (Children)
        for chunk_slice in chunks {
            // ZERO-COPY: We wrap the slice reference directly.
            let shard_inner = Box::new(VecShardJob { data: chunk_slice });

            // CONFIGURATION PROPAGATION:
            // It is critical to apply the vector configuration (e.g., LZ4 Compression) to the Shards,
            // as this is where 99% of the bytes reside.
            let shard_job: Box<dyn SerializationJob<'a> + 'a> = if let Some(cfg) = config_override {
                Box::new(crate::rt::ConfiguredJob::new(shard_inner, cfg))
            } else {
                shard_inner
            };

            let shard_id = graph.add_node(shard_job);
            graph.link_parent_child(my_id, shard_id);

            // Recursion to individual items.
            // Note: We pass 'None' as config override.
            // Reason: Items T are serialized within the Shard payload using Bincode.
            // They are not independent graph nodes (unless T explicitly creates sub-nodes).
            // If T is a complex struct, its own configuration (via Macro) will dictate how it behaves.
            for item in chunk_slice {
                // Recursion propagates the graph reference
                item.visit(graph, Some(shard_id), None);
            }
        }
    }

    fn create_job<'a>(
        &'a self,
        config_override: Option<JobConfig>,
    ) -> Box<dyn SerializationJob<'a> + 'a> {
        let inner = Box::new(VecContainerJob {
            shard_runs: Vec::new(),
            total_items: 0,
        });
        if let Some(cfg) = config_override {
            Box::new(crate::rt::ConfiguredJob::new(inner, cfg))
        } else {
            inner
        }
    }
}

// --- Implementation for Primitives ---

#[derive(Clone)]
struct PrimitiveJob<T>(T);

// Primitives own their data (copy), so they are valid for any lifetime 'a
impl<'a, T> SerializationJob<'a> for PrimitiveJob<T>
where
    T: Serialize + Send + Sync + Clone + 'static,
{
    fn execute(&self, _: &[ChildRef]) -> Result<Vec<u8>> {
        bincode::serde::encode_to_vec(&self.0, bincode::config::standard())
            .map_err(|e| ParcodeError::Serialization(e.to_string()))
    }
    fn estimated_size(&self) -> usize {
        std::mem::size_of::<T>()
    }
}

impl<T: ParcodeVisitor> ParcodeVisitor for &T {
    fn visit<'a>(
        &'a self,
        graph: &mut TaskGraph<'a>,
        parent_id: Option<ChunkId>,
        config_override: Option<JobConfig>,
    ) {
        (**self).visit(graph, parent_id, config_override)
    }

    fn create_job<'a>(
        &'a self,
        config_override: Option<JobConfig>,
    ) -> Box<dyn SerializationJob<'a> + 'a> {
        (**self).create_job(config_override)
    }
}

/// Macro to implement ParcodeVisitor for primitive types massively.
macro_rules! impl_primitive_visitor {
    ($($t:ty),*) => {
        $(
            impl ParcodeVisitor for $t {
                fn visit<'a>(&'a self, graph: &mut TaskGraph<'a>, parent_id: Option<ChunkId>, config_override: Option<JobConfig>) {
                    if parent_id.is_none() {
                        let job = self.create_job(config_override);
                        graph.add_node(job);
                    }
                }

                fn create_job<'a>(&'a self, config_override: Option<JobConfig>) -> Box<dyn SerializationJob<'a> + 'a> {
                    let base_job = Box::new(PrimitiveJob(self.clone()));
                    if let Some(cfg) = config_override {
                        Box::new(crate::rt::ConfiguredJob::new(base_job, cfg))
                    } else {
                        base_job
                    }
                }
            }
        )*
    }
}

impl_primitive_visitor!(
    u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, f32, f64, bool, String
);
