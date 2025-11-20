//! Pluggable compression backend.
//!
//! Handles the transformation of raw byte buffers into compressed chunks.
//! Includes conditional logic to skip compression for small payloads.

use crate::error::Result;
use std::borrow::Cow;

/// Threshold in bytes. If data is smaller than this, we don't compress.
#[allow(dead_code)]
const MIN_COMPRESSION_THRESHOLD: usize = 64;

/// Interface for compression algorithms.
pub trait Compressor: Send + Sync + std::fmt::Debug {
    /// Returns the unique ID stored in the MetaByte (Bits 1-3).
    /// 0 is reserved for No-Compression.
    fn id(&self) -> u8;

    /// Compresses the data.
    /// Implementations should respect `MIN_COMPRESSION_THRESHOLD`.
    fn compress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>>;

    /// Decompresses the data.
    fn decompress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>>;
}

// --- No Compression (Pass-through) ---

/// A compressor that performs no compression (pass-through).
#[derive(Debug, Clone, Copy)]
pub struct NoCompression;

impl Compressor for NoCompression {
    fn id(&self) -> u8 {
        0
    }

    fn compress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        Ok(Cow::Borrowed(data))
    }

    fn decompress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        Ok(Cow::Borrowed(data))
    }
}

// --- LZ4 Implementation (Optional) ---

#[cfg(feature = "lz4_flex")]
#[derive(Debug, Clone, Copy)]
pub struct Lz4Compressor;

#[cfg(feature = "lz4_flex")]
impl Compressor for Lz4Compressor {
    fn id(&self) -> u8 {
        1
    }

    fn compress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        // Heuristic: Don't compress tiny chunks
        if data.len() < MIN_COMPRESSION_THRESHOLD {
            return Ok(Cow::Borrowed(data));
        }

        let compressed = lz4_flex::compress_prepend_size(data);

        // Safety check: If compression made it larger (rare but possible on entropy),
        // return original. (Requires external logic to handle ID change,
        // for now we stick to the compressed version to keep ID stable).
        Ok(Cow::Owned(compressed))
    }

    fn decompress(&self, data: &[u8]) -> Result<Vec<u8>> {
        lz4_flex::decompress_size_prepended(data)
            .map_err(|e| crate::error::ParcodeError::Compression(e.to_string()))
    }
}
