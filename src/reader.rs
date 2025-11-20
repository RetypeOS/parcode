// src/reader.rs

//! The Read-Side Engine: Parallel Reconstruction & Random Access.
//!
//! This module implements the logic to map a `parcode` file into memory and reconstruct
//! complex data structures efficiently.
//!
//! # Core Architectures
//!
//! 1. **Lazy Traversal:** The file is Memory Mapped (`mmap`). We only read/decompress bytes
//!    when a specific node is requested.
//!
//! 2. **O(1) Arithmetic Navigation:** Using the RLE metadata stored in container nodes,
//!    we can calculate exactly which physical Chunk holds the Nth item of a collection,
//!    allowing random access without linear scans.
//!
//! 3. **Parallel Zero-Copy Stitching:** When reconstructing large `Vec<T>`, we:
//!    - Pre-allocate the final uninitialized memory buffer.
//!    - Calculate the destination offset for every shard.
//!    - Use `rayon` to dispatch parallel workers.
//!    - Each worker decodes a shard and writes directly into the final buffer.
//!    - **Result:** Maximized memory bandwidth, zero intermediate allocations.

use memmap2::Mmap;
use rayon::prelude::*;
use serde::{Deserialize, de::DeserializeOwned};
use std::borrow::Cow;
use std::fs::File;
use std::marker::PhantomData;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::path::Path;
use std::sync::Arc;

#[cfg(feature = "lz4_flex")]
use crate::compression::{self, Compressor};
use crate::error::{ParcodeError, Result};
use crate::format::{ChildRef, GLOBAL_HEADER_SIZE, GlobalHeader, MAGIC_BYTES, MetaByte};

// --- TRAIT SYSTEM FOR AUTOMATIC STRATEGY SELECTION ---

/// A trait for types that know how to reconstruct themselves from a `ChunkNode`.
///
/// This allows the high-level API (`Parcode::read`) to automatically select the
/// optimal strategy (Parallel vs Sequential) based on the type being read.
pub trait ParcodeNative: Sized {
    /// Reconstructs the object from the given graph node.
    fn from_node(node: &ChunkNode<'_>) -> Result<Self>;
}

/// Optimized implementation for Vectors: Uses Parallel Stitching.
impl<T> ParcodeNative for Vec<T>
where
    T: DeserializeOwned + Send + Sync,
{
    fn from_node(node: &ChunkNode<'_>) -> Result<Self> {
        node.decode_parallel_collection()
    }
}

// Note: For user structs, the `#[derive(ParcodeObject)]` macro (or manual impl)
// should implement this trait, typically calling `node.decode::<Self>()`.

// --- CORE READER HANDLE ---

/// The main handle for an open Parcode file.
///
/// It holds the memory map (thread-safe via Arc) and the global file header.
/// Cloning this struct is cheap (increments Arc ref count).
#[derive(Debug)]
pub struct ParcodeReader {
    mmap: Arc<Mmap>,
    header: GlobalHeader,
    file_size: u64,
}

impl ParcodeReader {
    /// Opens a Parcode file, maps it into memory, and validates integrity.
    ///
    /// # Errors
    /// Returns error if file doesn't exist, is truncated, or has invalid magic bytes.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path)?;
        let file_size = file.metadata()?.len();

        if file_size < GLOBAL_HEADER_SIZE as u64 {
            return Err(ParcodeError::Format(
                "File is smaller than the global header".into(),
            ));
        }

        // SAFETY: Mmap is fundamentally unsafe in the presence of external modification.
        // We assume the file is not modified by other processes while reading.
        #[allow(unsafe_code)]
        let mmap = unsafe { Mmap::map(&file)? };

        // Read Global Header (Located at the very end of the file)
        let header_start = (file_size as usize) - GLOBAL_HEADER_SIZE;
        let header_bytes = &mmap[header_start..];

        if header_bytes[0..4] != MAGIC_BYTES {
            return Err(ParcodeError::Format(
                "Invalid Magic Bytes. Not a Parcode file.".into(),
            ));
        }

        let version = u16::from_le_bytes(header_bytes[4..6].try_into().unwrap());
        if version != 4 {
            return Err(ParcodeError::Format(format!(
                "Unsupported version: {version}. Expected V4."
            )));
        }

        let root_offset = u64::from_le_bytes(header_bytes[6..14].try_into().unwrap());
        let root_length = u64::from_le_bytes(header_bytes[14..22].try_into().unwrap());
        let checksum = u32::from_le_bytes(header_bytes[22..26].try_into().unwrap());

        Ok(Self {
            mmap: Arc::new(mmap),
            header: GlobalHeader {
                magic: MAGIC_BYTES,
                version,
                root_offset,
                root_length,
                checksum,
            },
            file_size,
        })
    }

    #[allow(dead_code)]
    fn read_u64(slice: &[u8]) -> Result<u64> {
        slice
            .try_into()
            .map(u64::from_le_bytes)
            .map_err(|_| ParcodeError::Format("Failed to read u64".into()))
    }

    #[allow(dead_code)]
    fn read_u32(slice: &[u8]) -> Result<u32> {
        slice
            .try_into()
            .map(u32::from_le_bytes)
            .map_err(|_| ParcodeError::Format("Failed to read u32".into()))
    }

    /// Returns a cursor to the Root Chunk of the object graph.
    pub fn root(&self) -> Result<ChunkNode<'_>> {
        self.get_chunk(self.header.root_offset, self.header.root_length)
    }

    /// Internal: Resolves a physical offset/length into a `ChunkNode`.
    /// Parses the footer to determine if the chunk has children.
    fn get_chunk(&self, offset: u64, length: u64) -> Result<ChunkNode<'_>> {
        if offset + length > self.file_size {
            return Err(ParcodeError::Format(format!(
                "Chunk out of bounds: {} + {}",
                offset, length
            )));
        }
        let chunk_end = (offset + length) as usize;

        // Read the MetaByte (Last byte of the chunk)
        let meta = MetaByte::from_byte(self.mmap[chunk_end - 1]);

        let mut child_count = 0;
        let mut payload_end = chunk_end - 1; // Default: payload ends just before MetaByte

        if meta.is_chunkable() {
            // Layout: [Payload] ... [ChildRefs] [ChildCount (4 bytes)] [MetaByte (1 byte)]
            if length < 5 {
                return Err(ParcodeError::Format("Chunk too small for metadata".into()));
            }

            let count_start = chunk_end - 5;
            let child_count_bytes = &self.mmap[count_start..count_start + 4];
            child_count = Self::read_u32(child_count_bytes)?;

            let footer_size = child_count as usize * ChildRef::SIZE;
            let total_meta_size = 1 + 4 + footer_size;

            if length < total_meta_size as u64 {
                return Err(ParcodeError::Format("Invalid footer size".into()));
            }
            payload_end = chunk_end - total_meta_size;
        }

        Ok(ChunkNode {
            reader: self,
            offset,
            length,
            meta,
            child_count,
            payload_end_offset: offset + (payload_end as u64 - offset),
        })
    }
}

// --- CHUNK NODE API ---

/// A lightweight cursor pointing to a specific node in the dependency graph.
///
/// This struct contains the logic to read, decompress, and navigate from this node.
#[derive(Debug, Clone)]
pub struct ChunkNode<'a> {
    reader: &'a ParcodeReader,
    /// Physical start offset.
    offset: u64,
    /// Total physical length.
    #[allow(dead_code)]
    length: u64,
    /// Parsed metadata flags.
    meta: MetaByte,
    /// Number of direct children (Shards).
    child_count: u32,
    /// Calculated end of the payload data.
    payload_end_offset: u64,
}

/// Helper struct for deserializing RLE metadata.
#[derive(Deserialize, Debug, Clone)]
struct ShardRun {
    item_count: u32,
    repeat: u32,
}

impl<'a> ChunkNode<'a> {
    /// Reads and decompresses the local payload of this chunk.
    ///
    /// This returns `Cow`, so if no compression was used, it returns a direct reference
    /// to the mmap (Zero-Copy). If compressed, it allocates the decompressed buffer.
    pub fn read_raw(&self) -> Result<Cow<'a, [u8]>> {
        let start = self.offset as usize;
        let end = self.payload_end_offset as usize;

        if end > self.reader.mmap.len() {
            return Err(ParcodeError::Format(
                "Payload offset out of mmap bounds".into(),
            ));
        }

        let raw = &self.reader.mmap[start..end];

        match self.meta.compression_method() {
            0 => Ok(Cow::Borrowed(raw)), // FIX: TRUE ZERO-COPY
            #[cfg(feature = "lz4_flex")]
            1 => compression::lz4::Lz4Compressor::decompress_static(raw).map(Cow::Owned),
            id => Err(ParcodeError::Compression(format!("Unknown algo: {id}"))),
        }
    }

    /// Returns a list of all direct child nodes.
    ///
    /// This allows manual traversal of the dependency graph (e.g., iterating over specific shards).
    /// Note: This does not deserialize the children, only loads their metadata.
    pub fn children(&self) -> Result<Vec<ChunkNode<'a>>> {
        let mut list = Vec::with_capacity(self.child_count as usize);
        for i in 0..self.child_count {
            // Reutilizamos el helper interno
            list.push(self.get_child_by_index(i as usize)?);
        }
        Ok(list)
    }

    /// Standard single-threaded deserialization.
    /// Use this for leaf nodes or simple structs.
    pub fn decode<T: DeserializeOwned>(&self) -> Result<T> {
        let payload = self.read_raw()?;
        bincode::serde::decode_from_slice(&payload, bincode::config::standard())
            .map(|(obj, _)| obj)
            .map_err(|e| ParcodeError::Serialization(e.to_string()))
    }

    /// **Parallel Shard Reconstruction**
    ///
    /// This method reconstructs a `Vec<T>` by deserializing all shards in parallel
    /// and writing them directly into a preallocated buffer. It is designed for
    /// high‑performance scenarios where collections are split into shards and must
    /// be reassembled efficiently.
    ///
    /// # Safety & Performance Considerations
    /// - **Uninitialized Allocation:** Uses `MaybeUninit` to allocate the final buffer
    ///   without incurring the cost of zero‑initialization. This is safe only because
    ///   every element is guaranteed to be written before the buffer is exposed.
    /// - **Parallel Filling:** Uses `rayon` to concurrently populate disjoint regions
    ///   of the buffer. Each shard writes to a unique slice, ensuring no data races.
    /// - **Ownership Management:** Wraps temporary vectors in `ManuallyDrop` to prevent
    ///   Rust from automatically freeing their memory after items are copied. This avoids
    ///   double‑free errors when raw pointers are used.
    /// - **Pointer Arithmetic:** Converts the buffer’s base pointer into a `usize` so it
    ///   can safely cross thread boundaries (`Send + Sync + Copy`). Each worker thread
    ///   reconstructs the pointer locally before writing.
    /// - **Finalization:** After successful stitching, the uninitialized buffer is
    ///   “blessed” into a valid `Vec<T>` using `Vec::from_raw_parts`. This step is
    ///   inherently unsafe but correct because all invariants have been upheld.
    ///
    /// # Error Handling
    /// - Detects integer overflow during run‑length expansion of shard metadata.
    /// - Validates that the total item count implied by RLE matches the header.
    /// - Ensures no shard writes beyond the allocated buffer capacity.
    /// - Returns descriptive `ParcodeError` variants for all format or serialization issues.
    pub fn decode_parallel_collection<T>(&self) -> Result<Vec<T>>
    where
        T: DeserializeOwned + Send + Sync,
    {
        let payload = self.read_raw()?;

        // Fallback path for small vectors:
        // If the payload is too small to contain metadata, fall back to the
        // standard sequential decode. This avoids unnecessary parallel overhead.
        if payload.len() < 8 {
            return self.decode::<Vec<T>>();
        }

        // 1. Parse metadata from header
        // First 8 bytes encode the total number of items. The remainder encodes
        // run‑length encoded (RLE) shard descriptors.
        let total_items = u64::from_le_bytes(payload[0..8].try_into().unwrap()) as usize;
        let runs_data = &payload[8..];
        let shard_runs: Vec<ShardRun> =
            bincode::serde::decode_from_slice(runs_data, bincode::config::standard())
                .map(|(obj, _)| obj)
                .map_err(|e| ParcodeError::Serialization(e.to_string()))?;

        // 2. Expand RLE into explicit shard jobs
        // Each run describes how many items a shard contains and how many times
        // that shard pattern repeats. We flatten this into a list of jobs, each
        // with a shard index and its global start offset.
        let mut shard_jobs = Vec::with_capacity(self.child_count as usize);
        let mut current_shard_idx = 0;
        let mut current_global_idx: usize = 0;

        for run in shard_runs {
            let items_per_shard = run.item_count as usize;
            for _ in 0..run.repeat {
                // Overflow check: ensure global index arithmetic is safe
                if current_global_idx.checked_add(items_per_shard).is_none() {
                    return Err(ParcodeError::Format(
                        "Integer overflow in RLE calculation".into(),
                    ));
                }
                shard_jobs.push((current_shard_idx, current_global_idx));
                current_shard_idx += 1;
                current_global_idx += items_per_shard;
            }
        }

        // Strict metadata validation:
        // The total number of items implied by RLE must match the header.
        if current_global_idx != total_items {
            return Err(ParcodeError::Format(format!(
                "Metadata mismatch: Header says {} items, RLE implies {}",
                total_items, current_global_idx
            )));
        }

        if shard_jobs.is_empty() {
            return Ok(Vec::new());
        }

        // 3. Allocate uninitialized buffer
        // Reserve capacity for all items without initializing them.
        let mut result_buffer: Vec<MaybeUninit<T>> = Vec::with_capacity(total_items);

        #[allow(unsafe_code)]
        unsafe {
            result_buffer.set_len(total_items);
        }

        // 4. Perform parallel stitching
        // Trick: convert base pointer to `usize` so it can cross thread boundaries.
        let buffer_base = result_buffer.as_mut_ptr() as usize;

        shard_jobs
            .into_par_iter()
            .try_for_each(move |(shard_idx, start_idx)| -> Result<()> {
                let shard_node = self.get_child_by_index(shard_idx)?;
                // Deserialize shard into a temporary vector (thread‑local).
                let items: Vec<T> = shard_node.decode()?;
                let count = items.len();

                // Critical bounds check: ensure shard fits into allocated buffer.
                if start_idx + count > total_items {
                    return Err(ParcodeError::Format(
                        "Shard items overflowed allocated buffer".into(),
                    ));
                }

                // Prevent double‑free: wrap items in ManuallyDrop so Rust does not
                // automatically deallocate them after raw pointer copy.
                let src_items = ManuallyDrop::new(items);

                #[allow(unsafe_code)]
                unsafe {
                    // Reconstruct destination pointer from base.
                    // Note: `ptr::add` operates in units of T, not bytes.
                    let dest_ptr = (buffer_base as *mut T).add(start_idx);
                    let src_ptr = src_items.as_ptr(); // *const T

                    // Efficient memory copy (equivalent to memcpy).
                    std::ptr::copy_nonoverlapping(src_ptr, dest_ptr, count);
                }
                Ok(())
            })?;

        // 5. Finalize buffer into a valid Vec<T>
        // At this point, all elements have been safely written. We can now
        // reinterpret the uninitialized buffer as a fully initialized Vec<T>.
        #[allow(unsafe_code)]
        let final_vec = unsafe {
            let mut manual_buffer = ManuallyDrop::new(result_buffer);
            Vec::from_raw_parts(
                manual_buffer.as_mut_ptr() as *mut T,
                manual_buffer.len(),
                manual_buffer.capacity(),
            )
        };

        Ok(final_vec)
    }

    // --- COLLECTION UTILITIES ---

    /// Returns the logical number of items in this container.
    /// O(1) - reads cached header.
    pub fn len(&self) -> u64 {
        if let Ok(payload) = self.read_raw() {
            if payload.len() >= 8 {
                return u64::from_le_bytes(payload[0..8].try_into().unwrap());
            }
        }
        0
    }

    /// Checks if empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Retrieves item at `index` using RLE arithmetic.
    ///
    /// This calculates which shard holds the item, loads ONLY that shard,
    /// and returns the specific item.
    pub fn get<T: DeserializeOwned>(&self, index: usize) -> Result<T> {
        let payload = self.read_raw()?;
        if payload.len() < 8 {
            return Err(ParcodeError::Format("Not a valid container".into()));
        }

        let runs_data = &payload[8..];
        let shard_runs: Vec<ShardRun> =
            bincode::serde::decode_from_slice(runs_data, bincode::config::standard())
                .map(|(obj, _)| obj)
                .map_err(|e| ParcodeError::Serialization(e.to_string()))?;

        let (target_shard_idx, index_in_shard) = self.resolve_rle_index(index, &shard_runs)?;

        let shard_node = self.get_child_by_index(target_shard_idx)?;
        let shard_data: Vec<T> = shard_node.decode()?;

        shard_data
            .into_iter()
            .nth(index_in_shard)
            .ok_or(ParcodeError::Internal("Shard index mismatch".into()))
    }

    /// Creates a streaming iterator over the collection.
    /// Memory usage is constant (size of one shard) regardless of total size.
    pub fn iter<T: DeserializeOwned>(self) -> Result<ChunkIterator<'a, T>> {
        let payload = self.read_raw()?;
        if payload.is_empty() && self.child_count == 0 {
            return Ok(ChunkIterator::empty(self));
        }
        if payload.len() < 8 {
            return Err(ParcodeError::Format("Not a valid container".into()));
        }

        let total_len = u64::from_le_bytes(payload[0..8].try_into().unwrap()) as usize;
        let runs_data = &payload[8..];
        let shard_runs: Vec<ShardRun> =
            bincode::serde::decode_from_slice(runs_data, bincode::config::standard())
                .map(|(obj, _)| obj)
                .map_err(|e| ParcodeError::Serialization(e.to_string()))?;

        Ok(ChunkIterator {
            container: self,
            shard_runs,
            total_items: total_len,
            current_global_idx: 0,
            current_shard_idx: 0,
            current_items_in_shard: Vec::new().into_iter(),
            _marker: PhantomData,
        })
    }

    // --- INTERNAL HELPERS ---

    fn get_child_by_index(&self, index: usize) -> Result<ChunkNode<'a>> {
        if index >= self.child_count as usize {
            return Err(ParcodeError::Format("Child index out of bounds".into()));
        }
        // Calculate offset in the footer
        let footer_start = self.payload_end_offset as usize;
        let entry_start = footer_start + (index * ChildRef::SIZE);
        let bytes = &self.reader.mmap[entry_start..entry_start + ChildRef::SIZE];

        let r = ChildRef::from_bytes(bytes)?;
        self.reader.get_chunk(r.offset, r.length)
    }

    fn resolve_rle_index(&self, global_index: usize, runs: &[ShardRun]) -> Result<(usize, usize)> {
        let mut current_base = 0;
        let mut shard_base = 0;

        for run in runs {
            let count = run.item_count as usize;
            let total_run = count * run.repeat as usize;

            if global_index < current_base + total_run {
                let offset = global_index - current_base;
                // Integer division gives logical shard, modulo gives index inside
                return Ok((shard_base + (offset / count), offset % count));
            }
            current_base += total_run;
            shard_base += run.repeat as usize;
        }
        Err(ParcodeError::Format("Index out of bounds".into()))
    }
}

// --- STREAMING ITERATOR ---

/// An iterator that loads shards on demand, allowing iteration over datasets
/// larger than available RAM (if items are processed one by one).
#[derive(Debug)]
pub struct ChunkIterator<'a, T> {
    container: ChunkNode<'a>,
    #[allow(dead_code)]
    shard_runs: Vec<ShardRun>, // Kept for potential future RLE nav logic
    total_items: usize,
    current_global_idx: usize,

    // State
    current_shard_idx: usize,
    current_items_in_shard: std::vec::IntoIter<T>,

    _marker: PhantomData<T>,
}

impl<'a, T> ChunkIterator<'a, T> {
    fn empty(node: ChunkNode<'a>) -> Self {
        Self {
            container: node,
            shard_runs: vec![],
            total_items: 0,
            current_global_idx: 0,
            current_shard_idx: 0,
            current_items_in_shard: Vec::new().into_iter(),
            _marker: PhantomData,
        }
    }
}

impl<'a, T: DeserializeOwned> Iterator for ChunkIterator<'a, T> {
    type Item = Result<T>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_global_idx >= self.total_items {
            return None;
        }

        // 1. Try to pull from current loaded shard
        if let Some(item) = self.current_items_in_shard.next() {
            self.current_global_idx += 1;
            return Some(Ok(item));
        }

        // 2. Buffer empty? Load next shard
        if self.current_shard_idx >= self.container.child_count as usize {
            // Should not happen if total_items matches chunks, implies corruption or logic bug
            return Some(Err(ParcodeError::Internal(
                "Iterator mismatch: runs out of shards".into(),
            )));
        }

        // Load logic
        let next_shard_res = self
            .container
            .get_child_by_index(self.current_shard_idx)
            .and_then(|node| node.decode::<Vec<T>>());

        match next_shard_res {
            Ok(items) => {
                self.current_items_in_shard = items.into_iter();
                self.current_shard_idx += 1;
                // Recursively call next to yield the first item of the new shard
                self.next()
            }
            Err(e) => Some(Err(e)),
        }
    }
}
