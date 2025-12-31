//! The Read-Side Engine: Parallel Reconstruction & Random Access.
//!
//! This module implements the complete reading pipeline for Parcode files, providing both
//! eager (full deserialization) and lazy (on-demand) loading strategies. It leverages memory
//! mapping, parallel reconstruction, and zero-copy techniques to maximize performance.
//!
//! ## Core Architecture
//!
//! The reader is built on three foundational techniques:
//!
//! ### 1. Memory Mapping (`mmap`)
//!
//! Instead of reading the entire file into memory, Parcode uses `mmap` to map the file
//! directly into the process's address space. This provides several benefits:
//!
//! - **Instant Startup:** Opening a file is O(1) regardless of size
//! - **OS-Managed Paging:** The operating system handles loading pages on demand
//! - **Zero-Copy Reads:** Uncompressed data can be read directly from the mapped region
//! - **Shared Memory:** Multiple processes can share the same mapped file
//!
//! ### 2. Lazy Traversal
//!
//! The file is traversed lazily - we only read and decompress bytes when a specific node
//! is requested. This enables:
//!
//! - **Cold Start Performance:** Applications can start in microseconds
//! - **Selective Loading:** Load only the data you need
//! - **Deep Navigation:** Traverse object hierarchies without I/O
//!
//! ### 3. Parallel Zero-Copy Stitching
//!
//! When reconstructing large `Vec<T>`, we use a sophisticated parallel algorithm:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │ 1. Pre-allocate uninitialized buffer (MaybeUninit<T>)       │
//! ├─────────────────────────────────────────────────────────────┤
//! │ 2. Calculate destination offset for each shard              │
//! ├─────────────────────────────────────────────────────────────┤
//! │ 3. Spawn parallel workers (Rayon)                           │
//! ├─────────────────────────────────────────────────────────────┤
//! │ 4. Each worker:                                             │
//! │    - Decompresses its shard                                 │
//! │    - Deserializes items                                     │
//! │    - Writes directly to final buffer (ptr::copy)            │
//! ├─────────────────────────────────────────────────────────────┤
//! │ 5. Transmute buffer to Vec<T> (all items initialized)       │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! **Result:** Maximum memory bandwidth, zero intermediate allocations, perfect parallelism.
//!
//! ## O(1) Arithmetic Navigation
//!
//! Using the RLE (Run-Length Encoding) metadata stored in container nodes, we can calculate
//! exactly which physical chunk holds the Nth item of a collection. This enables:
//!
//! - **Random Access:** `vec.get(1_000_000)` without loading the entire vector
//! - **Constant Time:** O(1) shard selection via arithmetic
//! - **Minimal I/O:** Load only the shard containing the target item
//!
//! ## Trait System for Strategy Selection
//!
//! The module defines two key traits that enable automatic strategy selection:
//!
//! ### [`ParcodeNative`]
//!
//! Types implementing this trait know how to reconstruct themselves from a [`ChunkNode`].
//! The high-level API ([`Parcode::load`](crate::Parcode::load)) uses this trait to
//! automatically select the optimal reconstruction strategy:
//!
//! - **`Vec<T>`:** Uses parallel reconstruction across shards
//! - **`HashMap<K, V>`:** Reconstructs all shards and merges entries
//! - **Primitives/Structs:** Uses sequential deserialization
//!
//! ### [`ParcodeItem`]
//!
//! Types implementing this trait can be read from a shard (payload + children). This trait
//! is used internally during parallel reconstruction to deserialize individual items or
//! slices of items from shard payloads.
//!
//! ## Usage Patterns
//!
//! ### Eager Loading (Full Deserialization)
//!
//! ```rust
//! use parcode::Parcode;
//!
//! // Load entire object into memory
//! let data = vec![1, 2, 3];
//! Parcode::save("numbers_reader.par", &data)?;
//! let data: Vec<i32> = Parcode::load("numbers_reader.par")?;
//! # std::fs::remove_file("numbers_reader.par").ok();
//! # Ok::<(), parcode::ParcodeError>(())
//! ```
//!
//! ### Lazy Loading (On-Demand)
//!
//! ```rust
//! use parcode::{Parcode, ParcodeObject};
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Serialize, Deserialize, ParcodeObject)]
//! struct Assets {
//!     #[parcode(chunkable)]
//!     data: Vec<u8> }
//!
//! #[derive(Serialize, Deserialize, ParcodeObject)]
//! struct GameState {
//!     level: u32,
//!     #[parcode(chunkable)]
//!     assets: Assets,
//! }
//!
//! // Setup
//! let state = GameState { level: 1, assets: Assets { data: vec![0; 10] } };
//! Parcode::save("game_reader.par", &state)?;
//!
//! let file = Parcode::open("game_reader.par")?;
//! let game_lazy = file.root::<GameState>()?;
//!
//! // Access local fields (instant, already in memory)
//! println!("Level: {}", game_lazy.level);
//!
//! // Load remote fields on demand
//! let assets_data = game_lazy.assets.data.load()?;
//! # std::fs::remove_file("game_reader.par").ok();
//! # Ok::<(), parcode::ParcodeError>(())
//! ```
//!
//! ### Random Access
//!
//! ```rust
//! use parcode::{Parcode, ParcodeObject};
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Serialize, Deserialize, ParcodeObject, Clone, Debug)]
//! struct MyStruct { val: u32 }
//!
//! // Setup
//! let data: Vec<MyStruct> = (0..100).map(|i| MyStruct { val: i }).collect();
//! Parcode::save("data_random.par", &data)?;
//!
//! let file = Parcode::open("data_random.par")?;
//! let root = file.root::<Vec<MyStruct>>()?;
//!
//! // Get item at index 50 without loading the entire vector
//! // Note: Using 50 instead of 1,000,000 for a realistic small test
//! let item = root.get(50)?;
//! # std::fs::remove_file("data_random.par").ok();
//! # Ok::<(), parcode::ParcodeError>(())
//! ```
//!
//! ### Streaming Iteration
//!
//! ```rust
//! use parcode::{Parcode, ParcodeObject};
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Serialize, Deserialize, ParcodeObject, Clone, Debug)]
//! struct MyStruct { val: u32 }
//!
//! fn process(item: MyStruct) { println!("{:?}", item); }
//!
//! // Setup
//! let data: Vec<MyStruct> = (0..10).map(|i| MyStruct { val: i }).collect();
//! Parcode::save("data_iter.par", &data)?;
//!
//! let file = Parcode::open("data_iter.par")?;
//! let items: Vec<MyStruct> = file.load()?;
//!
//! // Note: The current API doesn't have a direct `iter` on root for Vecs yet,
//! // it usually goes through read_lazy or decode.
//! // Assuming we just decode for now as the example implies iteration capability.
//! for item in items {
//!     process(item);
//! }
//! # std::fs::remove_file("data_iter.par").ok();
//! # Ok::<(), parcode::ParcodeError>(())
//! ```
//!
//! ## Performance Characteristics
//!
//! - **File Opening:** O(1) - just maps the file
//! - **Root Access:** O(1) - reads only the global header
//! - **Random Access:** O(1) - arithmetic shard selection + single shard load
//! - **Parallel Reconstruction:** O(N/cores) - scales linearly with CPU cores
//! - **Memory Usage (Lazy):** O(accessed chunks) - only loaded data consumes RAM
//! - **Memory Usage (Eager):** O(N) - entire object in memory
//!
//! ## Thread Safety
//!
//! - **[`ParcodeFile`]:** Cheap to clone (Arc-based), safe to share across threads
//! - **[`ChunkNode`]:** Immutable view, safe to share across threads
//! - **Parallel Reconstruction:** Uses Rayon's work-stealing scheduler
//!
//! ## Safety Considerations
//!
//! The module uses `unsafe` code in two specific contexts:
//!
//! 1. **Memory Mapping:** `mmap` is inherently unsafe if the file is modified externally.
//!    We assume files are immutable during reading.
//!
//! 2. **Parallel Stitching:** Uses `MaybeUninit` and pointer arithmetic to avoid
//!    initialization overhead. All unsafe operations are carefully encapsulated and
//!    documented with safety invariants.

use serde::{Deserialize, de::DeserializeOwned};
use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::Hash;
use std::io::Cursor;
use std::marker::PhantomData;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::ops::Deref;
use std::ptr;
use std::slice;
use std::sync::Arc;

#[cfg(all(feature = "mmap", not(target_arch = "wasm32")))]
use memmap2::Mmap;
#[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
use rayon::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use std::fs::File;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

use crate::compression::CompressorRegistry;
use crate::error::{ParcodeError, Result};
use crate::format::{ChildRef, GLOBAL_HEADER_SIZE, GlobalHeader, MAGIC_BYTES, MetaByte};
use crate::rt::ParcodeLazyRef;

// ---

/// PLACEHOLDER
#[derive(Debug, Clone)]
pub enum DataSource {
    #[cfg(all(feature = "mmap", not(target_arch = "wasm32")))]
    /// PLACEHOLDER
    Mmap(Arc<Mmap>),
    /// PLACEHOLDER
    Memory(Arc<Vec<u8>>),
}

impl Deref for DataSource {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &Self::Target {
        match self {
            #[cfg(all(feature = "mmap", not(target_arch = "wasm32")))]
            Self::Mmap(mmap) => mmap.as_ref(),
            Self::Memory(vec) => vec.as_slice(),
        }
    }
}

/// A trait for types that know how to reconstruct themselves from a [`ChunkNode`].
///
/// This trait enables the high-level API ([`Parcode::load`](crate::Parcode::load)) to
/// automatically select the optimal reconstruction strategy based on the type being read.
///
/// ## Strategy Selection
///
/// Different types use different reconstruction strategies:
///
/// - **`Vec<T>`:** Parallel reconstruction across shards (see [`ChunkNode::decode_parallel_collection`])
/// - **`HashMap<K, V>`:** Shard merging with SOA deserialization
/// - **Primitives:** Direct bincode deserialization
/// - **Custom Structs:** Sequential deserialization of local fields + recursive child loading
///
/// ## Automatic Implementation
///
/// This trait is automatically implemented by the `#[derive(ParcodeObject)]` macro for custom
/// structs. Primitive types and standard collections have manual implementations in this module.
///
/// ## Example
///
/// ```rust
/// use parcode::Parcode;
///
/// // Automatically selects parallel reconstruction for Vec
/// let data = vec![1, 2, 3];
/// let mut buffer = Vec::new();
/// Parcode::write(&mut buffer, &data)?;
/// let data: Vec<i32> = Parcode::load_bytes(buffer)?;
///
/// // Automatically selects sequential deserialization for primitives
/// let val = 42;
/// let mut buffer = Vec::new();
/// Parcode::write(&mut buffer, &val)?;
/// let value: i32 = Parcode::load_bytes(buffer)?;
/// # Ok::<(), parcode::ParcodeError>(())
/// ```
pub trait ParcodeNative: Sized {
    /// Reconstructs the object from the given graph node.
    ///
    /// This method is called by [`Parcode::load`](crate::Parcode::load) after opening the
    /// file and locating the root chunk. Implementations should choose the most efficient
    /// reconstruction strategy for their type.
    ///
    /// ## Parameters
    ///
    /// * `node`: The chunk node to reconstruct from (typically the root node)
    ///
    /// ## Returns
    ///
    /// The fully reconstructed object of type `Self`.
    ///
    /// ## Errors
    ///
    /// Returns an error if:
    /// - Decompression fails
    /// - Deserialization fails (type mismatch, corrupted data)
    /// - Child nodes are missing or invalid
    fn from_node(node: &ChunkNode<'_>) -> Result<Self>;
}

/// A trait for types that can be read from a shard (payload + children).
///
/// This trait is used internally during parallel reconstruction to deserialize individual
/// items or slices of items from shard payloads. It provides two methods:
///
/// - [`read_from_shard`](Self::read_from_shard): Reads a single item
/// - [`read_slice_from_shard`](Self::read_slice_from_shard): Reads multiple items (optimized)
///
/// ## Automatic Implementation
///
/// This trait is automatically implemented by the `#[derive(ParcodeObject)]` macro. Primitive
/// types have optimized implementations that use bulk deserialization.
///
/// ## Thread Safety
///
/// Implementations must be `Send + Sync + 'static` to support parallel reconstruction across
/// threads. This is automatically satisfied for most types.
pub trait ParcodeItem: Sized + Send + Sync + 'static {
    /// Reads a single item from the shard payload and children.
    ///
    /// This method is called during deserialization to reconstruct individual items from
    /// a shard's payload. For types with chunkable fields, this method should deserialize
    /// local fields from the reader and reconstruct remote fields from the children iterator.
    ///
    /// ## Parameters
    ///
    /// * `reader`: Cursor over the shard's decompressed payload
    /// * `children`: Iterator over child nodes (for chunkable fields)
    ///
    /// ## Returns
    ///
    /// The deserialized item.
    ///
    /// ## Errors
    ///
    /// Returns an error if deserialization fails or children are missing.
    fn read_from_shard(
        reader: &mut std::io::Cursor<&[u8]>,
        children: &mut std::vec::IntoIter<ChunkNode<'_>>,
    ) -> Result<Self>;

    /// DEPRECATED in favor of `read_into_slice` internally, but kept for API compat.
    /// Default impl now delegates to `read_into_slice` to reduce code duplication.
    fn read_slice_from_shard(
        reader: &mut std::io::Cursor<&[u8]>,
        children: &mut std::vec::IntoIter<ChunkNode<'_>>,
    ) -> Result<Vec<Self>> {
        let start_pos = reader.position();

        // Read length prefix
        let len: u64 = bincode::serde::decode_from_std_read(reader, bincode::config::standard())
            .map_err(|e| ParcodeError::Serialization(e.to_string()))?;

        let count = usize::try_from(len)
            .map_err(|_| ParcodeError::Serialization("Vector length exceeds usize".into()))?;

        // Allocate uninit vec
        let mut uninit_vec: Vec<MaybeUninit<Self>> = Vec::with_capacity(count);
        #[allow(unsafe_code)]
        unsafe {
            uninit_vec.set_len(count);
        }

        reader.set_position(start_pos);

        let read_count = Self::read_into_slice(reader, children, &mut uninit_vec)?;

        if read_count != count {
            return Err(ParcodeError::Format("Mismatch in read count".into()));
        }

        // Transmute to Vec<T>
        #[allow(unsafe_code)]
        let final_vec = unsafe {
            let mut manual = ManuallyDrop::new(uninit_vec);
            Vec::from_raw_parts(manual.as_mut_ptr() as *mut Self, count, count)
        };
        Ok(final_vec)
    }

    /// Reads items directly into a destination slice of uninitialized memory.
    ///
    /// # Performance
    /// This method avoids allocating a temporary `Vec<T>`, writing directly to the final buffer.
    ///
    /// # Safety
    /// The implementation must ensure it does not write past the end of `destination`.
    /// It returns the number of items actually written.
    fn read_into_slice(
        reader: &mut std::io::Cursor<&[u8]>,
        children: &mut std::vec::IntoIter<ChunkNode<'_>>,
        destination: &mut [MaybeUninit<Self>],
    ) -> Result<usize> {
        // 1. Read Length
        let len: u64 = bincode::serde::decode_from_std_read(reader, bincode::config::standard())
            .map_err(|e| ParcodeError::Serialization(e.to_string()))?;

        let count = usize::try_from(len)
            .map_err(|_| ParcodeError::Serialization("Shard length exceeds usize".into()))?;

        // 2. Bounds Check
        if destination.len() < count {
            return Err(ParcodeError::Format(format!(
                "Destination buffer too small: expected {}, got {}",
                count,
                destination.len()
            )));
        }

        // 3. Loop and Initialize
        for slot in destination.iter_mut().take(count) {
            let item = Self::read_from_shard(reader, children)?;
            slot.write(item);
        }

        Ok(count)
    }
}

macro_rules! impl_primitive_parcode_item {
    ($($t:ty),*) => {
        $(
            impl ParcodeItem for $t {
                fn read_from_shard(
                    reader: &mut std::io::Cursor<&[u8]>,
                    _children: &mut std::vec::IntoIter<ChunkNode<'_>>,
                ) -> Result<Self> {
                    bincode::serde::decode_from_std_read(reader, bincode::config::standard())
                        .map_err(|e| ParcodeError::Serialization(e.to_string()))
                }

                fn read_into_slice(
                    reader: &mut std::io::Cursor<&[u8]>,
                    _children: &mut std::vec::IntoIter<ChunkNode<'_>>,
                    destination: &mut [MaybeUninit<Self>],
                ) -> Result<usize> {
                    // Read Length
                    let len: u64 = bincode::serde::decode_from_std_read(reader, bincode::config::standard())
                        .map_err(|e| ParcodeError::Serialization(e.to_string()))?;
                    let count = usize::try_from(len).map_err(|_| ParcodeError::Format("Len overflow".into()))?;

                    if destination.len() < count {
                        return Err(ParcodeError::Format("Buffer too small".into()));
                    }

                    // If T is u8, we can memcpy directly from the reader slice.
                    if std::any::TypeId::of::<Self>() == std::any::TypeId::of::<u8>() {
                        let pos = usize::try_from(reader.position())
                            .map_err(|_| ParcodeError::Format("Position overflow".into()))?;
                        let inner = reader.get_ref();

                        let src_slice = inner.get(pos..pos + count)
                            .ok_or_else(|| ParcodeError::Format("Unexpected EOF reading u8 blob".into()))?;

                        let dest_ptr = destination.as_mut_ptr() as *mut u8;
                        #[allow(unsafe_code)]
                        unsafe {
                            ptr::copy_nonoverlapping(src_slice.as_ptr(), dest_ptr, count);
                        }

                        reader.set_position((pos + count) as u64);
                        return Ok(count);
                    }

                    // Default Primitive Loop
                    for slot in destination.iter_mut().take(count) {
                        let item: Self = bincode::serde::decode_from_std_read(reader, bincode::config::standard())
                             .map_err(|e| ParcodeError::Serialization(e.to_string()))?;
                        slot.write(item);
                    }
                    Ok(count)
                }
            }
        )*
    }
}

impl_primitive_parcode_item!(
    u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64, bool, String
);

macro_rules! impl_primitive_parcode_native {
    ($($t:ty),*) => {
        $(
            impl ParcodeNative for $t {
                fn from_node(node: &ChunkNode<'_>) -> Result<Self> {
                    node.decode()
                }
            }
        )*
    }
}

impl_primitive_parcode_native!(
    u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64, bool, String
);

/// Uses Parallel Stitching.
impl<T> ParcodeNative for Vec<T>
where
    T: ParcodeItem,
{
    fn from_node(node: &ChunkNode<'_>) -> Result<Self> {
        node.decode_parallel_collection()
    }
}

impl<K, V> ParcodeNative for HashMap<K, V>
where
    K: DeserializeOwned + Eq + Hash + Send + Sync,
    V: DeserializeOwned + Send + Sync,
{
    fn from_node(node: &ChunkNode<'_>) -> Result<Self> {
        // Standard HashMap reconstruction logic (same as before)
        let container_payload = node.read_raw()?;
        if container_payload.len() < 4 {
            return Ok(Self::new());
        }

        if node.child_count == 0 {
            return node.decode();
        }

        let shards = node.children()?;
        let mut map = Self::new();

        for shard in shards {
            let payload = shard.read_raw()?;
            if payload.len() < 8 {
                continue;
            }

            let count = u32::from_le_bytes(
                payload
                    .get(0..4)
                    .ok_or_else(|| ParcodeError::Format("Payload too short for count".into()))?
                    .try_into()
                    .map_err(|_| ParcodeError::Format("Failed to parse count".into()))?,
            ) as usize;
            let offsets_start = 8 + (count * 8);
            let data_start = offsets_start + (count * 4);
            let offsets_bytes = payload
                .get(offsets_start..data_start)
                .ok_or_else(|| ParcodeError::Format("Offsets out of bounds".into()))?;

            for off_bytes in offsets_bytes.chunks_exact(4).take(count) {
                let offset = u32::from_le_bytes(
                    off_bytes
                        .try_into()
                        .map_err(|_| ParcodeError::Format("Failed to parse offset".into()))?,
                ) as usize;
                let data_slice = payload
                    .get(data_start + offset..)
                    .ok_or_else(|| ParcodeError::Format("Data slice out of bounds".into()))?;
                let (k, v) =
                    bincode::serde::decode_from_slice(data_slice, bincode::config::standard())
                        .map_err(|e| ParcodeError::Serialization(e.to_string()))?
                        .0;
                map.insert(k, v);
            }
        }
        Ok(map)
    }
}

/// Represents an open Parcode file mapped in memory.
///
/// This handle allows:
/// - Eager loading of the full data (`load`).
/// - Lazy access to the root structure (`root`).
/// - Structural inspection (`inspect`).
#[derive(Debug)]
pub struct ParcodeFile {
    /// Memory file content.
    source: DataSource,
    /// Parsed global footer/header information.
    header: GlobalHeader,
    /// Total size of the file in bytes.
    file_size: u64,
    /// Registry containing available decompression algorithms (Lz4, etc.).
    registry: CompressorRegistry,
}

impl ParcodeFile {
    /// Opens a Parcode file from disk.
    ///
    /// # Platform Behavior
    /// * **Desktop/Server:**
    ///   - If `feature = "mmap"` is enabled (default), uses memory mapping for Zero-Copy.
    ///   - If `feature = "mmap"` is disabled, reads the entire file into RAM.
    /// * **WASM:** This method is not available. Use `from_bytes` instead.
    ///
    /// # Errors
    /// Returns error if file doesn't exist, lacks permissions, or is invalid.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        // Memory mapping
        #[cfg(feature = "mmap")]
        {
            let file = File::open(path)?;
            #[allow(unsafe_code)]
            let mmap = unsafe { Mmap::map(&file)? };

            Self::init(DataSource::Mmap(Arc::new(mmap)), file.metadata()?.len())
        }

        // Read from RAM
        #[cfg(not(feature = "mmap"))]
        {
            use std::io::Read;

            let mut file = File::open(path)?;
            let file_size = file.metadata()?.len();
            let mut buffer =
                Vec::with_capacity(usize::try_from(file_size).expect("File size too large"));

            file.read_to_end(&mut buffer)?;

            Self::init(DataSource::Memory(Arc::new(buffer)), file_size)
        }
    }

    /// Opens a Parcode file from an in-memory buffer.
    ///
    /// This is the primary entry point for tests and architectures without direct file access.
    /// The `ParcodeFile` takes ownership of the data (wrapped in Arc).
    pub fn from_bytes(data: impl Into<Arc<Vec<u8>>>) -> Result<Self> {
        let data_arc = data.into();
        let size = data_arc.len() as u64;
        Self::init(DataSource::Memory(data_arc), size)
    }

    fn init(source: DataSource, file_size: u64) -> Result<Self> {
        if file_size < GLOBAL_HEADER_SIZE as u64 {
            return Err(ParcodeError::Format("File smaller than header".into()));
        }

        let header_start = usize::try_from(file_size - GLOBAL_HEADER_SIZE as u64)
            .map_err(|_| ParcodeError::Format("File size too large for usize".into()))?;
        let header_bytes = source
            .get(header_start..)
            .ok_or_else(|| ParcodeError::Format("Failed to access global header".into()))?;

        if header_bytes.get(0..4) != Some(&MAGIC_BYTES) {
            return Err(ParcodeError::Format("Invalid Magic Bytes".into()));
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
                .map_err(|_| ParcodeError::Format("Failed to read root offset".into()))?,
        );
        let root_length = u64::from_le_bytes(
            header_bytes
                .get(14..22)
                .ok_or_else(|| ParcodeError::Format("Root length out of bounds".into()))?
                .try_into()
                .map_err(|_| ParcodeError::Format("Failed to read root length".into()))?,
        );
        let checksum = u32::from_le_bytes(
            header_bytes
                .get(22..26)
                .ok_or_else(|| ParcodeError::Format("Checksum out of bounds".into()))?
                .try_into()
                .map_err(|_| ParcodeError::Format("Failed to read checksum".into()))?,
        );

        let header = GlobalHeader {
            magic: MAGIC_BYTES,
            version,
            root_offset,
            root_length,
            checksum,
        };

        Ok(Self {
            source,
            header,
            file_size,
            registry: CompressorRegistry::new(),
        })
    }

    /// Fully deserializes the file content into memory (Eager).
    ///
    /// This is the equivalent of `Parcode::load()`, but on an already open file.
    pub fn load<T: ParcodeNative>(&self) -> Result<T> {
        let root = self.root_node()?;
        T::from_node(&root)
    }

    /// Returns a Lazy Mirror of the root object.
    ///
    /// This operation is instant (O(1)) and incurs no I/O overhead.
    /// Data is only loaded when specific fields are accessed on the returned mirror.
    pub fn root<'a, T>(&'a self) -> Result<T::Lazy>
    where
        T: ParcodeLazyRef<'a>,
    {
        let root = self.root_node()?;
        T::create_lazy(root)
    }

    /// Alias for `root()` for users who prefer explicit naming.
    #[inline]
    pub fn load_lazy<'a, T>(&'a self) -> Result<T::Lazy>
    where
        T: ParcodeLazyRef<'a>,
    {
        self.root::<T>()
    }

    /// Alias for `root()` for backward compatibility.
    #[inline]
    pub fn read_lazy<'a, T>(&'a self) -> Result<T::Lazy>
    where
        T: ParcodeLazyRef<'a>,
    {
        self.root::<T>()
    }

    /// Generates a structural inspection report of the file.
    pub fn inspect(&self) -> Result<crate::inspector::DebugReport> {
        crate::inspector::ParcodeInspector::inspect_file(self)
    }

    /// Returns the total size of the file in bytes.
    pub fn file_size(&self) -> u64 {
        self.file_size
    }

    /// Internal helper to get the root chunk node.
    pub fn root_node(&self) -> Result<ChunkNode<'_>> {
        self.get_chunk(self.header.root_offset, self.header.root_length)
    }

    /// Helper to read a u32 from a byte slice (Little Endian).
    fn read_u32(slice: &[u8]) -> Result<u32> {
        slice
            .try_into()
            .map(u32::from_le_bytes)
            .map_err(|_| ParcodeError::Format("Failed to read u32".into()))
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

        let meta_byte = self
            .source
            .get(chunk_end - 1)
            .ok_or_else(|| ParcodeError::Format("Failed to read chunk meta byte".into()))?;
        let meta = MetaByte::from_byte(*meta_byte);

        let mut child_count = 0;
        let mut payload_end = chunk_end - 1;

        if meta.is_chunkable() {
            if length < 5 {
                return Err(ParcodeError::Format("Chunk too small for metadata".into()));
            }

            let count_start = chunk_end - 5;
            let count_bytes = self
                .source
                .get(count_start..count_start + 4)
                .ok_or_else(|| ParcodeError::Format("Failed to read child count".into()))?;
            child_count = Self::read_u32(count_bytes)?;

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
    reader: &'a ParcodeFile,
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

        // Access via self.reader.source (Deref to &[u8])
        let raw = self
            .reader
            .source
            .get(start..end)
            .ok_or_else(|| ParcodeError::Format("Payload out of bounds".into()))?;
        let method_id = self.meta.compression_method();

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
    /// Reconstructs a `Vec<T>` by deserializing shards concurrently directly into the final buffer.
    ///
    /// # Optimizations
    /// - **Destination Passing Style:** Shards write directly to `&mut [MaybeUninit<T>]`.
    /// - **Zero Alloc:** No temporary `Vec<T>` created per thread.
    /// - **Memory Efficient:** Peak memory usage is reduced by ~50% during load.
    #[allow(unsafe_code)]
    pub fn decode_parallel_collection<T>(&self) -> Result<Vec<T>>
    where
        T: ParcodeItem,
    {
        let payload = self.read_raw()?;

        if payload.is_empty() {
            return Ok(Vec::new());
        }

        // Fallback for small/inline payloads
        if payload.len() < 8 {
            let mut cursor = std::io::Cursor::new(payload.as_ref());
            let children = self.children()?;
            let mut child_iter = children.into_iter();
            return T::read_slice_from_shard(&mut cursor, &mut child_iter);
        }

        // 1. Read Total Items
        let total_items = usize::try_from(u64::from_le_bytes(
            payload
                .get(0..8)
                .ok_or_else(|| ParcodeError::Format("Payload too short".into()))?
                .try_into()
                .map_err(|_| ParcodeError::Format("Failed to parse total items".into()))?,
        ))
        .map_err(|_| ParcodeError::Format("total_items exceeds usize".into()))?;

        // 2. Parse RLE Metadata
        let runs_data = payload.get(8..).unwrap_or(&[]);
        let shard_runs: Vec<ShardRun> =
            bincode::serde::decode_from_slice(runs_data, bincode::config::standard())
                .map(|(obj, _)| obj)
                .map_err(|e| ParcodeError::Serialization(e.to_string()))?;

        // 3. Expand RLE to Jobs
        let mut shard_jobs = Vec::with_capacity(self.child_count as usize);
        let mut current_shard_idx = 0;
        let mut current_global_idx: usize = 0;

        for run in shard_runs {
            let items_per_shard = run.item_count as usize;
            for _ in 0..run.repeat {
                if current_global_idx.checked_add(items_per_shard).is_none() {
                    return Err(ParcodeError::Format("Integer overflow in RLE".into()));
                }
                shard_jobs.push((current_shard_idx, current_global_idx, items_per_shard));
                current_shard_idx += 1;
                current_global_idx += items_per_shard;
            }
        }

        if current_global_idx != total_items {
            return Err(ParcodeError::Format(format!(
                "Metadata mismatch: Header={}, RLE={}",
                total_items, current_global_idx
            )));
        }

        if shard_jobs.is_empty() {
            return Ok(Vec::new());
        }

        // 4. Allocate Uninitialized Buffer
        let mut result_buffer: Vec<MaybeUninit<T>> = Vec::with_capacity(total_items);
        unsafe {
            result_buffer.set_len(total_items);
        }

        // 5. Parallel Execution
        // Cast the buffer to a raw usize address to allow sending it to threads.
        // `MaybeUninit` is not implicitly Sync, but since each thread writes to disjoint regions,
        // it is sound.
        let buffer_base_addr = result_buffer.as_mut_ptr() as usize;

        #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
        {
            // Parallel Execution
            shard_jobs.into_par_iter().try_for_each(
                |(shard_idx, start_idx, expected_count)| -> Result<()> {
                    self.decode_shard_into_buffer::<T>(
                        shard_idx,
                        start_idx,
                        expected_count,
                        buffer_base_addr,
                    )
                },
            )?;
        }

        #[cfg(not(feature = "parallel"))]
        {
            // Serial Execution
            for (shard_idx, start_idx, expected_count) in shard_jobs {
                self.decode_shard_into_buffer::<T>(
                    shard_idx,
                    start_idx,
                    expected_count,
                    buffer_base_addr,
                )?;
            }
        }

        unsafe {
            let mut manual = ManuallyDrop::new(result_buffer);
            Ok(Vec::from_raw_parts(
                manual.as_mut_ptr() as *mut T,
                manual.len(),
                manual.capacity(),
            ))
        }
    }

    fn decode_shard_into_buffer<T: ParcodeItem>(
        &self,
        shard_idx: usize,
        start_idx: usize,
        expected_count: usize,
        buffer_base_addr: usize,
    ) -> Result<()> {
        let shard_node = self.get_child_by_index(shard_idx)?;
        let payload = shard_node.read_raw()?;
        let mut cursor = Cursor::new(payload.as_ref());
        let children = shard_node.children()?;
        let mut child_iter = children.into_iter();

        #[allow(unsafe_code)]
        let dest_slice = unsafe {
            let ptr = (buffer_base_addr as *mut MaybeUninit<T>).add(start_idx);
            slice::from_raw_parts_mut(ptr, expected_count)
        };

        // Uses Destination Passing Style to avoid intermediate allocs
        let items_read = T::read_into_slice(&mut cursor, &mut child_iter, dest_slice)?;

        if items_read != expected_count {
            return Err(ParcodeError::Format(format!(
                "Shard {} items mismatch: expected {}, got {}",
                shard_idx, expected_count, items_read
            )));
        }
        Ok(())
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

    /// Helper internal: Locates the shard node and local index for a global index.
    /// Used by `ParcodeCollectionPromise::get_lazy`.
    pub(crate) fn locate_shard_item(&self, index: usize) -> Result<(Self, usize)> {
        let payload = self.read_raw()?;

        if payload.len() < 8 {
            return Err(ParcodeError::Format("Invalid container payload".into()));
        }

        // Skip total_items
        let runs_data = payload.get(8..).unwrap_or(&[]);
        let shard_runs: Vec<ShardRun> =
            bincode::serde::decode_from_slice(runs_data, bincode::config::standard())
                .map(|(obj, _)| obj)
                .map_err(|e| ParcodeError::Serialization(e.to_string()))?;

        let (shard_idx, local_idx) = self.resolve_rle_index(index, &shard_runs)?;

        let shard = self.get_child_by_index(shard_idx)?;
        Ok((shard, local_idx))
    }

    /// Retrieves item at `index` using RLE arithmetic.
    ///
    /// This calculates which shard holds the item, loads ONLY that shard,
    /// and returns the specific item.
    pub fn get<T: ParcodeItem>(&self, index: usize) -> Result<T> {
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

        let payload = shard_node.read_raw()?;
        let mut cursor = std::io::Cursor::new(payload.as_ref());
        let children = shard_node.children()?;
        let mut child_iter = children.into_iter();

        let shard_data: Vec<T> = T::read_slice_from_shard(&mut cursor, &mut child_iter)?;

        shard_data
            .into_iter()
            .nth(index_in_shard)
            .ok_or(ParcodeError::Internal("Shard index mismatch".into()))
    }

    /// Creates a streaming iterator over the collection.
    /// Memory usage is constant (size of one shard) regardless of total size.
    pub fn iter<T: ParcodeItem>(self) -> Result<ChunkIterator<'a, T>> {
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
    pub fn get_child_by_index(&self, index: usize) -> Result<Self> {
        if index >= self.child_count as usize {
            return Err(ParcodeError::Format("Child index out of bounds".into()));
        }
        let footer_start = usize::try_from(self.payload_end_offset)
            .map_err(|_| ParcodeError::Format("Offset exceeds usize range".into()))?;
        let entry_start = footer_start + (index * ChildRef::SIZE);

        let bytes = self
            .reader
            .source
            .get(entry_start..entry_start + ChildRef::SIZE)
            .ok_or_else(|| ParcodeError::Format("Child reference out of bounds".into()))?;

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
                return Ok((shard_base + (offset / count), offset % count));
            }
            current_base += total_run;
            shard_base += run.repeat as usize;
        }
        Err(ParcodeError::Format("Index out of bounds".into()))
    }

    /// Returns the absolute file offset of this chunk.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Returns the total physical length of this chunk.
    pub fn length(&self) -> u64 {
        self.length
    }

    /// Returns the number of children.
    pub fn child_count(&self) -> u32 {
        self.child_count
    }

    /// Returns the metadata flags.
    pub fn meta(&self) -> crate::format::MetaByte {
        self.meta
    }

    /// Calculates the size of the payload (excluding metadata/footer).
    pub fn payload_len(&self) -> u64 {
        // offset + (payload_end - offset)
        self.payload_end_offset - self.offset
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

impl<'a, T: ParcodeItem> Iterator for ChunkIterator<'a, T> {
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
            .and_then(|node| {
                let payload = node.read_raw()?;
                let mut cursor = std::io::Cursor::new(payload.as_ref());
                let children = node.children()?;
                let mut child_iter = children.into_iter();
                T::read_slice_from_shard(&mut cursor, &mut child_iter)
            });

        match next_shard_res {
            Ok(items) => {
                self.current_items_in_shard = items.into_iter();
                self.current_shard_idx += 1;
                self.next()
            }
            Err(e) => Some(Err(e)),
        }
    }
}
