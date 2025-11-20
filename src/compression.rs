//! Pluggable compression backend.
//!
//! Handles the transformation of raw byte buffers into compressed chunks.

use std::borrow::Cow;
use crate::error::{Result, ParcodeError};

/// Threshold in bytes. (Removed logic usage, kept for reference or future smart heuristics outside the impl)
#[allow(dead_code)] 
const MIN_COMPRESSION_THRESHOLD: usize = 64;

/// Interface for compression algorithms.
pub trait Compressor: Send + Sync + std::fmt::Debug {
    /// Returns the unique ID stored in the MetaByte (Bits 1-3).
    /// 0 is reserved for No-Compression.
    fn id(&self) -> u8;

    /// Compresses the data.
    fn compress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>>;

    /// Decompresses the data.
    fn decompress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>>;
}

// --- No Compression (Pass-through) ---

/// A compressor that performs no compression (pass-through).
#[derive(Debug, Clone, Copy)]
pub struct NoCompression;

impl Compressor for NoCompression {
    fn id(&self) -> u8 { 0 }

    fn compress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        // Zero-copy: return reference to input
        Ok(Cow::Borrowed(data))
    }

    fn decompress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        // Zero-copy: return reference to mmap
        Ok(Cow::Borrowed(data))
    }
}

// --- LZ4 Implementation (Optional) ---

#[cfg(feature = "lz4_flex")]
/// PLACEHOLDER
#[derive(Debug, Clone, Copy)]
pub struct Lz4Compressor;

#[cfg(feature = "lz4_flex")]
impl Compressor for Lz4Compressor {
    fn id(&self) -> u8 { 1 }

    fn compress<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        // FIX CRÍTICO: Eliminada la optimización de retorno temprano para datos pequeños.
        // Si el Executor pide ID 1, debemos devolver formato LZ4 válido siempre.
        // De lo contrario, el Reader intentará descomprimir datos crudos y fallará.
        
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

/// Registro centralizado de algoritmos de compresión.
#[derive(Debug)]
pub struct CompressorRegistry {
    algorithms: Vec<Option<Box<dyn Compressor>>>,
}

impl CompressorRegistry {
    /// PLACEHOLDER
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

    /// PLACEHOLDER
    pub fn register(&mut self, algo: Box<dyn Compressor>) {
        let id = algo.id() as usize;
        if id >= self.algorithms.len() {
            self.algorithms.resize_with(id + 1, || None);
        }
        self.algorithms[id] = Some(algo);
    }

    /// PLACEHOLDER
    pub fn get(&self, id: u8) -> Result<&dyn Compressor> {
        let idx = id as usize;
        if idx < self.algorithms.len() {
            if let Some(algo) = &self.algorithms[idx] {
                return Ok(algo.as_ref());
            }
        }
        Err(ParcodeError::Compression(format!("Algorithm ID {} is not registered or available", id)))
    }
}

impl Default for CompressorRegistry {
    fn default() -> Self {
        Self::new()
    }
}