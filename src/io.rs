//! High-Performance Sequential Writer.
//!
//! # Architecture: Aggressive Buffering
//! Instead of complex channel passing (which introduces scheduling jitter),
//! we use a `Mutex` protecting a massive `BufWriter` (16MB).
//!
//! - **Latency:** Writes are effectively `memcpy` operations into RAM until the buffer fills.
//! - **Throughput:** Flushes happen in huge chunks, saturating SSD bandwidth.
//! - **Stability:** Eliminates channel backpressure "stop-and-go" behavior.

use crate::error::{ParcodeError, Result};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Mutex;

/// We use a 16MB buffer.
/// This allows ~128 chunks of 128KB to be written purely in memory
/// before triggering a syscall.
const WRITE_BUFFER_SIZE: usize = 16 * 1024 * 1024;

/// A thread-safe, buffered sequential writer.
///
/// This writer is designed to be shared across multiple threads (via `Mutex`)
/// to allow concurrent graph execution to serialize data, while ensuring
/// that the actual disk writes happen sequentially and efficiently.
#[derive(Debug)]
pub struct SeqWriter {
    inner: Mutex<WriterState>,
}

#[derive(Debug)]
struct WriterState {
    writer: BufWriter<File>,
    current_offset: u64,
}

impl SeqWriter {
    /// Opens the file with an optimized buffer configuration.
    ///
    /// The file is created (truncated if it exists) and wrapped in a large `BufWriter`.
    pub fn create(path: &Path) -> Result<Self> {
        let file = File::create(path)?;
        Ok(Self {
            inner: Mutex::new(WriterState {
                writer: BufWriter::with_capacity(WRITE_BUFFER_SIZE, file),
                current_offset: 0,
            }),
        })
    }

    /// Writes a chunk of data atomically to the file sequence.
    ///
    /// Returns the byte offset where the chunk begins.
    pub fn write_all(&self, buffer: &[u8]) -> Result<u64> {
        // Acquire lock.
        // Since we mostly do memcpy, contention is extremely low.
        let mut state = self
            .inner
            .lock()
            .map_err(|_| ParcodeError::Internal("Writer mutex poisoned".into()))?;

        let start_offset = state.current_offset;

        // This call usually just copies memory.
        // It only blocks for disk I/O once every ~16MB of data.
        state.writer.write_all(buffer)?;

        state.current_offset += buffer.len() as u64;

        Ok(start_offset)
    }

    /// Forces data to disk.
    pub fn flush(&self) -> Result<()> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| ParcodeError::Internal("Writer mutex poisoned".into()))?;
        state.writer.flush()?;
        Ok(())
    }
}
