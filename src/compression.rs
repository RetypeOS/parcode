//! Pluggable compression backend.
//!
//! Handles the transformation of raw byte buffers into compressed chunks.
//! This module defines the `Compressor` trait and a registry for managing
//! available compression algorithms.

use crate::error::{ParcodeError, Result};
use std::borrow::Cow;

/// Threshold in bytes. (Removed logic usage, kept for reference or future smart heuristics outside the impl)
#[allow(dead_code)]
const MIN_COMPRESSION_THRESHOLD: usize = 64;

/// Interface for compression algorithms.
///
/// Implementors of this trait provide the logic to compress and decompress
/// byte buffers. Each compressor is identified by a unique ID.
pub trait Compressor: Send + Sync + std::fmt::Debug {
    /// Returns the unique ID stored in the `MetaByte` (Bits 1-3).
    /// 0 is reserved for No-Compression.
    fn id(&self) -> u8;

    /// Compresses the data.
    ///
    /// Returns a `Cow<[u8]>` which may borrow the input if no compression is performed
    /// or if the compressed size is larger than the original.
    fn compress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>>;

    /// Decompresses the data.
    ///
    /// Returns a `Cow<[u8]>` containing the original data.
    fn decompress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>>;
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
