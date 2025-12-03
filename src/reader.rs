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

use crate::compression::CompressorRegistry;
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

// --- CORE READER HANDLE ---

/// The main handle for an open Parcode file.
///
/// It holds the memory map (thread-safe via Arc), the global file header,
/// and the registry of available decompression algorithms.
/// Cloning this struct is cheap (increments Arc ref count).
#[derive(Debug)]
pub struct ParcodeReader {
    /// Memory-mapped file content.
    mmap: Arc<Mmap>,
    /// Parsed global footer/header information.
    header: GlobalHeader,
    /// Total size of the file in bytes.
    file_size: u64,
    /// Registry containing available decompression algorithms (Lz4, etc.).
    registry: CompressorRegistry,
}

impl ParcodeReader {
    /// Opens a Parcode file, maps it into memory, and validates integrity.
    ///
    /// # Errors
    /// Returns error if the file does not exist, is smaller than the header,
    /// or contains invalid magic bytes/version.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path)?;
        let file_size = file.metadata()?.len();

        if file_size < GLOBAL_HEADER_SIZE as u64 {
            return Err(ParcodeError::Format(
                "File is smaller than the global header".into(),
            ));
        }

        // SAFETY: Mmap is fundamentally unsafe in the presence of external modification
        // (e.g., another process truncating the file). We assume the file is treated
        // as immutable by the OS while we read it.
        #[allow(unsafe_code)]
        let mmap = unsafe { Mmap::map(&file)? };

        // Read Global Header (Located at the very end of the file)
        let header_start = usize::try_from(file_size)
            .map_err(|_| ParcodeError::Format("File too large for address space".into()))?
            - GLOBAL_HEADER_SIZE;
        let header_bytes = mmap
            .get(header_start..)
            .ok_or_else(|| ParcodeError::Format("Header start out of bounds".into()))?;

        if header_bytes.get(0..4) != Some(&MAGIC_BYTES) {
            return Err(ParcodeError::Format(
                "Invalid Magic Bytes. Not a Parcode file.".into(),
            ));
        }

        let version = u16::from_le_bytes(
            header_bytes
                .get(4..6)
                .ok_or_else(|| ParcodeError::Format("Version out of bounds".into()))?
                .try_into()
                .map_err(|_| ParcodeError::Format("Failed to read version".into()))?,
        );
        if version != 4 {
            return Err(ParcodeError::Format(format!(
                "Unsupported version: {version}. Expected V4."
            )));
        }

        let root_offset = u64::from_le_bytes(
            header_bytes
                .get(6..14)
                .ok_or_else(|| ParcodeError::Format("Root offset out of bounds".into()))?
                .try_into()
                .map_err(|_| ParcodeError::Format("Failed to read root_offset".into()))?,
        );
        let root_length = u64::from_le_bytes(
            header_bytes
                .get(14..22)
                .ok_or_else(|| ParcodeError::Format("Root length out of bounds".into()))?
                .try_into()
                .map_err(|_| ParcodeError::Format("Failed to read root_length".into()))?,
        );
        let checksum = u32::from_le_bytes(
            header_bytes
                .get(22..26)
                .ok_or_else(|| ParcodeError::Format("Checksum out of bounds".into()))?
                .try_into()
                .map_err(|_| ParcodeError::Format("Failed to read checksum".into()))?,
        );

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
            // Initialize registry with default algorithms (NoCompression, Lz4 if enabled)
            registry: CompressorRegistry::new(),
        })
    }

    /// Helper to read a u32 from a byte slice (Little Endian).
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
    ///
    /// # Arguments
    /// * `offset`: Absolute byte offset in the file.
    /// * `length`: Total length of the chunk including metadata.
    fn get_chunk(&self, offset: u64, length: u64) -> Result<ChunkNode<'_>> {
        if offset + length > self.file_size {
            return Err(ParcodeError::Format(format!(
                "Chunk out of bounds: {} + {}",
                offset, length
            )));
        }
        let chunk_end = usize::try_from(offset + length)
            .map_err(|_| ParcodeError::Format("Chunk end exceeds address space".into()))?;

        // Read the MetaByte (Last byte of the chunk)
        let meta = MetaByte::from_byte(
            *self
                .mmap
                .get(chunk_end - 1)
                .ok_or_else(|| ParcodeError::Format("MetaByte out of bounds".into()))?,
        );

        let mut child_count = 0;
        let mut payload_end = chunk_end - 1; // Default: payload ends just before MetaByte

        if meta.is_chunkable() {
            // Layout: [Payload] ... [ChildRefs] [ChildCount (4 bytes)] [MetaByte (1 byte)]
            if length < 5 {
                return Err(ParcodeError::Format("Chunk too small for metadata".into()));
            }

            let count_start = chunk_end - 5;
            let child_count_bytes = self
                .mmap
                .get(count_start..count_start + 4)
                .ok_or_else(|| ParcodeError::Format("Child count out of bounds".into()))?;
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
/// It is a "view" into the `ParcodeReader` and holds a lifetime reference to it.
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

/// Helper struct for deserializing RLE metadata stored in vector headers.
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
        let start = usize::try_from(self.offset)
            .map_err(|_| ParcodeError::Format("Offset exceeds address space".into()))?;
        let end = usize::try_from(self.payload_end_offset)
            .map_err(|_| ParcodeError::Format("End offset exceeds address space".into()))?;

        if end > self.reader.mmap.len() {
            return Err(ParcodeError::Format(
                "Payload offset out of mmap bounds".into(),
            ));
        }

        let raw = self
            .reader
            .mmap
            .get(start..end)
            .ok_or_else(|| ParcodeError::Format("Payload out of bounds".into()))?;
        let method_id = self.meta.compression_method();

        // Delegate decompression to the registry.
        // This supports pluggable algorithms (e.g., Lz4).
        self.reader.registry.get(method_id)?.decompress(raw)
    }

    /// Returns a list of all direct child nodes.
    ///
    /// This allows manual traversal of the dependency graph (e.g., iterating over specific shards).
    /// Note: This does not deserialize the children, only loads their metadata (offsets).
    pub fn children(&self) -> Result<Vec<Self>> {
        let mut list = Vec::with_capacity(self.child_count as usize);
        for i in 0..self.child_count {
            list.push(self.get_child_by_index(i as usize)?);
        }
        Ok(list)
    }

    /// Standard single-threaded deserialization.
    /// Use this for leaf nodes or simple structs that fit in memory.
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
    /// high‑performance scenarios where collections are split into shards.
    ///
    /// # Safety & Performance Considerations
    /// - **Uninitialized Allocation:** Uses `MaybeUninit` to allocate the final buffer
    ///   without zero‑initialization cost.
    /// - **Parallel Filling:** Uses `rayon` to concurrently populate disjoint regions.
    /// - **Ownership Management:** Wraps temporary vectors in `ManuallyDrop` to prevent
    ///   double‑free errors when moving memory via `ptr::copy`.
    /// - **Pointer Arithmetic:** Converts the buffer base pointer to `usize` to safely
    ///   share it across thread boundaries (`Send + Sync + Copy`).
    pub fn decode_parallel_collection<T>(&self) -> Result<Vec<T>>
    where
        T: DeserializeOwned + Send + Sync,
    {
        let payload = self.read_raw()?;

        // Fallback path for small vectors or leaves:
        if payload.len() < 8 {
            return self.decode::<Vec<T>>();
        }

        // 1. Parse metadata from header
        let total_items = usize::try_from(u64::from_le_bytes(
            payload
                .get(0..8)
                .ok_or_else(|| ParcodeError::Format("Payload too short for header".into()))?
                .try_into()
                .map_err(|_| ParcodeError::Format("Failed to read total_items".into()))?,
        ))
        .map_err(|_| ParcodeError::Format("total_items exceeds usize range".into()))?;
        let runs_data = payload.get(8..).unwrap_or(&[]);
        let shard_runs: Vec<ShardRun> =
            bincode::serde::decode_from_slice(runs_data, bincode::config::standard())
                .map(|(obj, _)| obj)
                .map_err(|e| ParcodeError::Serialization(e.to_string()))?;

        // 2. Expand RLE into explicit shard jobs
        let mut shard_jobs = Vec::with_capacity(self.child_count as usize);
        let mut current_shard_idx = 0;
        let mut current_global_idx: usize = 0;

        for run in shard_runs {
            let items_per_shard = run.item_count as usize;
            for _ in 0..run.repeat {
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
        let mut result_buffer: Vec<MaybeUninit<T>> = Vec::with_capacity(total_items);

        // SAFETY: We are creating a "hole" in memory that we PROMISE to fill.
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
                // Deserialize shard into a thread-local vector
                let items: Vec<T> = shard_node.decode()?;
                let count = items.len();

                if start_idx + count > total_items {
                    return Err(ParcodeError::Format(
                        "Shard items overflowed allocated buffer".into(),
                    ));
                }

                // Prevent double‑free: wrap items so we can take ownership of bits
                let src_items = ManuallyDrop::new(items);

                #[allow(unsafe_code)]
                unsafe {
                    // Reconstruct pointer. `ptr::add` works on T units.
                    let dest_ptr = (buffer_base as *mut T).add(start_idx);
                    let src_ptr = src_items.as_ptr();

                    // Efficient memory copy
                    std::ptr::copy_nonoverlapping(src_ptr, dest_ptr, count);
                }
                Ok(())
            })?;

        // 5. Bless the buffer
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
    pub fn len(&self) -> u64 {
        if let Ok(payload) = self.read_raw()
            && payload.len() >= 8
            && let Some(bytes) = payload.get(0..8).and_then(|s| s.try_into().ok())
        {
            return u64::from_le_bytes(bytes);
        }
        0
    }

    /// Checks if the container is empty.
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

        let runs_data = payload.get(8..).unwrap_or(&[]);
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

        let total_len = usize::try_from(u64::from_le_bytes(
            payload
                .get(0..8)
                .ok_or_else(|| ParcodeError::Format("Payload too short".into()))?
                .try_into()
                .map_err(|_| ParcodeError::Format("Failed to read total_len".into()))?,
        ))
        .map_err(|_| ParcodeError::Format("total_len exceeds usize range".into()))?;
        let runs_data = payload.get(8..).unwrap_or(&[]);
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

    /// Retrieves a child `ChunkNode` by its index in the footer.
    fn get_child_by_index(&self, index: usize) -> Result<Self> {
        if index >= self.child_count as usize {
            return Err(ParcodeError::Format("Child index out of bounds".into()));
        }
        let footer_start = usize::try_from(self.payload_end_offset)
            .map_err(|_| ParcodeError::Format("Offset exceeds usize range".into()))?;
        let entry_start = footer_start + (index * ChildRef::SIZE);
        let bytes = self
            .reader
            .mmap
            .get(entry_start..entry_start + ChildRef::SIZE)
            .ok_or_else(|| ParcodeError::Format("ChildRef index out of bounds".into()))?;

        let r = ChildRef::from_bytes(bytes)?;
        self.reader.get_chunk(r.offset, r.length)
    }

    /// Maps a global item index to a specific (`shard_index`, `internal_index`).
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
/// larger than available RAM.
///
/// It buffers only one shard at a time.
#[derive(Debug)]
pub struct ChunkIterator<'a, T> {
    container: ChunkNode<'a>,
    #[allow(dead_code)]
    shard_runs: Vec<ShardRun>, // Reserved for future skip logic
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
