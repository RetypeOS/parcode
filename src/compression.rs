//! Pluggable compression backend for Parcode.
//!
//! This module provides a flexible compression system that allows different algorithms
//! to be used for compressing chunks. The design is based on a trait-based plugin
//! architecture with a central registry.
//!
//! ## Architecture
//!
//! The compression system consists of three main components:
//!
//! 1. **[`Compressor`] Trait:** Defines the interface that all compression algorithms
//!    must implement. Each compressor has a unique ID (0-7) that is stored in the
//!    chunk's `MetaByte`.
//!
//! 2. **[`CompressorRegistry`]:** A centralized registry that maps algorithm IDs to
//!    their implementations. The registry is used during both serialization (to compress)
//!    and deserialization (to decompress).
//!
//! 3. **Concrete Implementations:** Specific compression algorithms like [`NoCompression`]
//!    and `Lz4Compressor` (when the `lz4_flex` feature is enabled).
//!
//! ## Compression IDs
//!
//! Each compression algorithm is assigned a unique ID that fits in 3 bits (0-7):
//!
//! - **ID 0:** [`NoCompression`] (pass-through, always available)
//! - **ID 1:** `Lz4Compressor` (requires `lz4_flex` feature)
//! - **IDs 2-7:** Reserved for future algorithms (zstd, brotli, etc.)
//!
//! The ID is stored in bits 1-3 of the chunk's `MetaByte`, allowing the reader to
//! select the correct decompressor without external metadata.
//!
//! ## Design Rationale
//!
//! ### Why a Trait-Based System?
//!
//! - **Extensibility:** New compression algorithms can be added without modifying
//!   existing code
//! - **Feature Flags:** Algorithms can be conditionally compiled based on features
//! - **Testing:** Mock compressors can be injected for testing
//!
//! ### Why a Registry?
//!
//! - **Centralized Management:** Single source of truth for available algorithms
//! - **Runtime Selection:** The reader can dynamically select the decompressor
//!   based on the chunk's `MetaByte`
//! - **Error Handling:** Missing algorithms are detected early with clear errors
//!
//! ### Why Cow<[u8]>?
//!
//! The [`Compressor::compress`] method returns `Cow<[u8]>` to support zero-copy
//! in cases where compression is not beneficial:
//!
//! - If the compressed size ≥ original size, return `Cow::Borrowed` (no allocation)
//! - If compression helps, return `Cow::Owned` with the compressed data
//!
//! ## Usage
//!
//! ### Using the Default Registry
//!
//! ```rust
//! use parcode::compression::CompressorRegistry;
//!
//! let registry = CompressorRegistry::new();
//! let compressor = registry.get(0).unwrap(); // Get NoCompression
//! ```
//!
//! ### Compressing Data
//!
//! ```rust
//! use parcode::compression::{Compressor, NoCompression};
//!
//! let compressor = NoCompression;
//! let data = b"Hello, world!";
//! let compressed = compressor.compress(data).unwrap();
//! ```
//!
//! ### Registering Custom Compressors
//!
//! ```rust
//! use parcode::compression::{CompressorRegistry, NoCompression};
//! let mut registry = CompressorRegistry::new();
//! registry.register(Box::new(NoCompression));
//! ```
//!
//! ## Performance Considerations
//!
//! - **`NoCompression`:** Zero overhead (just a memcpy)
//! - **LZ4:** Fast compression (~500 MB/s) with moderate ratios (2-3x)
//! - **Threshold:** Small chunks (< 64 bytes) may not benefit from compression
//!
//! ## Thread Safety
//!
//! All compressors must implement `Send + Sync` to support parallel execution.
//! The registry itself is not `Sync` (it's built once and shared via `&`).

use crate::error::{ParcodeError, Result};
use std::borrow::Cow;

/// Minimum size threshold for compression consideration.
///
/// Chunks smaller than this size may not benefit from compression due to overhead.
/// This constant is kept for reference and potential future use in adaptive compression
/// strategies.
///
/// Currently not actively used in compression decisions, but reserved for future
/// smart heuristics that might skip compression for very small payloads.
#[allow(dead_code)]
const MIN_COMPRESSION_THRESHOLD: usize = 64;

/// Interface for compression algorithms.
///
/// This trait defines the contract that all compression implementations must fulfill.
/// Each compressor is identified by a unique ID (0-7) that is stored in the chunk's
/// `MetaByte`, allowing the reader to select the appropriate decompressor.
///
/// ## Design
///
/// The trait provides three methods:
///
/// 1. **[`id`](Self::id):** Returns the unique algorithm identifier
/// 2. **[`compress`](Self::compress):** Compresses data, returning `Cow` for zero-copy optimization
/// 3. **[`decompress`](Self::decompress):** Decompresses data, returning `Cow` for zero-copy when possible
/// 4. **[`compress_append`](Self::compress_append):** Compresses directly into a buffer (avoids intermediate allocation)
///
/// ## Thread Safety
///
/// Implementations must be `Send + Sync` to support parallel compression across multiple
/// threads. This is enforced by the trait bounds.
///
/// ## Implementing Custom Compressors
///
/// To add a new compression algorithm:
///
/// ```rust
/// use parcode::compression::Compressor;
/// use parcode::Result;
/// use std::borrow::Cow;
///
/// #[derive(Debug)]
/// struct MyCompressor;
///
/// impl Compressor for MyCompressor {
///     fn id(&self) -> u8 { 2 } // Use an available ID (2-7)
///     
///     fn compress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
///         // Implement compression logic
///         Ok(Cow::Borrowed(data))
///     }
///     
///     fn decompress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
///         // Implement decompression logic
///         Ok(Cow::Borrowed(data))
///     }
///     
///     fn compress_append(&self, data: &[u8], output: &mut Vec<u8>) -> Result<()> {
///         // Implement direct-to-buffer compression
///         output.extend_from_slice(data);
///         Ok(())
///     }
/// }
/// ```
///
/// ## Performance Notes
///
/// - The `compress_append` method is preferred during serialization as it avoids
///   intermediate allocations
/// - Returning `Cow::Borrowed` from `compress` when compression doesn't help
///   enables zero-copy optimization
pub trait Compressor: Send + Sync + std::fmt::Debug {
    /// Returns the unique ID stored in the `MetaByte` (Bits 1-3).
    ///
    /// This ID must be in the range 0-7 (3 bits) and should be unique across all
    /// registered compressors. ID 0 is reserved for [`NoCompression`].
    ///
    /// ## ID Allocation
    ///
    /// - **0:** `NoCompression` (reserved)
    /// - **1:** LZ4 (reserved)
    /// - **2-7:** Available for custom algorithms
    fn id(&self) -> u8;

    /// Compresses the input data.
    ///
    /// Returns a `Cow<[u8]>` which may borrow the input if compression is not performed
    /// or if the compressed size would be larger than the original (negative compression).
    ///
    /// ## Return Value
    ///
    /// - `Cow::Borrowed(data)`: Compression was not beneficial or not performed
    /// - `Cow::Owned(compressed)`: Data was successfully compressed
    ///
    /// ## Errors
    ///
    /// Returns [`ParcodeError::Compression`] if the compression algorithm fails.
    ///
    /// ## Performance
    ///
    /// This method may allocate memory for the compressed output. For better performance
    /// during serialization, use [`compress_append`](Self::compress_append) instead.
    fn compress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>>;

    /// Decompresses the input data.
    ///
    /// Returns a `Cow<[u8]>` containing the original uncompressed data.
    ///
    /// ## Return Value
    ///
    /// - `Cow::Borrowed(data)`: No decompression needed (e.g., `NoCompression`)
    /// - `Cow::Owned(decompressed)`: Data was successfully decompressed
    ///
    /// ## Errors
    ///
    /// Returns [`ParcodeError::Compression`] if:
    /// - The compressed data is corrupted
    /// - The decompression algorithm fails
    /// - The decompressed size exceeds expected bounds
    fn decompress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>>;

    /// Compresses data and appends it directly to the output vector.
    ///
    /// This method is more efficient than [`compress`](Self::compress) during serialization
    /// because it avoids intermediate allocations by writing directly to the final buffer.
    ///
    /// ## Parameters
    ///
    /// - `data`: The input data to compress
    /// - `output`: The buffer to append compressed data to
    ///
    /// ## Errors
    ///
    /// Returns [`ParcodeError::Compression`] if the compression algorithm fails.
    ///
    /// ## Performance
    ///
    /// This is the preferred method for compression during serialization as it:
    /// - Avoids intermediate allocations
    /// - Writes directly to the final output buffer
    /// - Reduces memory copies
    fn compress_append(&self, data: &[u8], output: &mut Vec<u8>) -> Result<()>;
}

// --- No Compression (Pass-through) ---

/// A compressor that performs no compression (pass-through).
///
/// This is the default strategy (ID 0). It simply passes the data through unchanged.
#[derive(Debug, Clone, Copy)]
pub struct NoCompression;

impl Compressor for NoCompression {
    fn id(&self) -> u8 {
        0
    }

    fn compress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        // Zero-copy: return reference to input
        Ok(Cow::Borrowed(data))
    }

    fn decompress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        // Zero-copy: return reference to mmap
        Ok(Cow::Borrowed(data))
    }

    fn compress_append(&self, data: &[u8], output: &mut Vec<u8>) -> Result<()> {
        output.extend_from_slice(data);
        Ok(())
    }
}

// --- LZ4 Implementation ---

#[cfg(feature = "lz4_flex")]
/// A compressor using the LZ4 algorithm.
///
/// This compressor is available when the `lz4_flex` feature is enabled.
/// It uses the `lz4_flex` crate for high-performance compression.
#[derive(Debug, Clone, Copy)]
pub struct Lz4Compressor;

#[cfg(feature = "lz4_flex")]
impl Compressor for Lz4Compressor {
    fn id(&self) -> u8 {
        1
    }

    fn compress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        let compressed = lz4_flex::compress_prepend_size(data);
        Ok(Cow::Owned(compressed))
    }

    fn decompress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        let vec = lz4_flex::decompress_size_prepended(data)
            .map_err(|e| ParcodeError::Compression(e.to_string()))?;
        Ok(Cow::Owned(vec))
    }

    fn compress_append(&self, data: &[u8], output: &mut Vec<u8>) -> Result<()> {
        // Reserve space for the worst-case compressed size + 4 bytes for size prefix
        // We can't easily use compress_prepend_size without allocation, so we implement it manually
        // to write directly to output.

        // Format: [u32 LE uncompressed_len] [compressed_data]
        // Wait, lz4_flex::compress_prepend_size uses a specific format.
        // It stores uncompressed size as u32 LE? No, it might be varint or just u32.
        // Checking lz4_flex docs (recalled): it uses u32 little endian.

        let uncompressed_len = u32::try_from(data.len())
            .map_err(|_| ParcodeError::Compression("Data too large for u32".to_string()))?;
        output.extend_from_slice(&uncompressed_len.to_le_bytes());

        let _start_idx = output.len();
        let max_size = lz4_flex::block::get_maximum_output_size(data.len());
        output.reserve(max_size);

        let current_len = output.len();
        output.resize(current_len + max_size, 0);

        let output_slice = output.get_mut(current_len..).ok_or_else(|| {
            ParcodeError::Compression("Output buffer resizing failed".to_string())
        })?;

        let result = lz4_flex::block::compress_into(data, output_slice);

        match result {
            Ok(bytes_written) => {
                output.truncate(current_len + bytes_written);
                Ok(())
            }
            Err(e) => {
                output.truncate(current_len); // Restore
                Err(ParcodeError::Compression(e.to_string()))
            }
        }
    }
}

// --- REGISTRY ---

/// Centralized registry for compression algorithms.
///
/// The registry maps algorithm IDs (stored in the file format) to
/// specific `Compressor` implementations.
#[derive(Debug)]
pub struct CompressorRegistry {
    algorithms: Vec<Option<Box<dyn Compressor>>>,
}

impl CompressorRegistry {
    /// Creates a new registry with default algorithms registered.
    ///
    /// *   ID 0: `NoCompression`
    /// *   ID 1: `Lz4Compressor` (if `lz4_flex` feature is enabled)
    pub fn new() -> Self {
        let mut reg = Self {
            algorithms: (0..8).map(|_| None).collect(),
        };

        // ID 0: NoCompression
        reg.register(Box::new(NoCompression));

        // ID 1: Lz4
        #[cfg(feature = "lz4_flex")]
        reg.register(Box::new(Lz4Compressor));

        reg
    }

    /// Registers a new compressor.
    ///
    /// The compressor's ID (returned by `algo.id()`) determines its slot in the registry.
    /// If a compressor with the same ID is already registered, it will be overwritten.
    pub fn register(&mut self, algo: Box<dyn Compressor>) {
        let id = algo.id() as usize;

        // Ensure the vector is large enough to hold the new ID.
        if id >= self.algorithms.len() {
            self.algorithms.resize_with(id + 1, || None);
        }

        // `resize_with` guarantees the index is valid.
        let slot = self
            .algorithms
            .get_mut(id)
            .expect("Registry vector resized but index not found. This is a bug.");

        *slot = Some(algo);
    }

    /// Retrieves a compressor by its ID.
    ///
    /// # Errors
    /// Returns `ParcodeError::Compression` if the ID is not registered.
    pub fn get(&self, id: u8) -> Result<&dyn Compressor> {
        let idx = usize::from(id);
        if idx < self.algorithms.len()
            && let Some(algo) = self.algorithms.get(idx).and_then(|opt| opt.as_ref())
        {
            return Ok(algo.as_ref());
        }

        Err(ParcodeError::Compression(format!(
            "Algorithm ID {} is not registered or available",
            id
        )))
    }
}

impl Default for CompressorRegistry {
    fn default() -> Self {
        Self::new()
    }
}
