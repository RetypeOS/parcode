//! Low-level I/O operations handling sequential writing.
//!
//! This module ensures that multiple threads can submit data to be written
//! to disk without race conditions, guaranteeing atomic writes of chunks.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::Mutex;
use crate::error::Result;

/// A thread-safe writer that appends data to a file and tracks the current offset.
#[derive(Debug)]
pub struct SeqWriter {
    /// The underlying buffered writer protected by a Mutex.
    /// We use a Mutex because writing to a file is inherently sequential.
    inner: Mutex<WriterState>,
}

#[derive(Debug)]
struct WriterState {
    writer: BufWriter<File>,
    current_offset: u64,
}

impl SeqWriter {
    /// Creates a new SeqWriter from an existing file.
    /// The file is truncated on creation.
    pub fn create(path: &std::path::Path) -> Result<Self> {
        let file = File::create(path)?;
        Ok(Self {
            inner: Mutex::new(WriterState {
                writer: BufWriter::new(file),
                current_offset: 0,
            }),
        })
    }

    /// Atomically writes a complete buffer to the file.
    /// Returns the offset where the writing started.
    pub fn write_all(&self, buffer: &[u8]) -> Result<u64> {
        // Lock the writer. This is the synchronization point.
        let mut state = self.inner.lock().map_err(|_| 
            crate::error::ParcodeError::Internal("SeqWriter Mutex poisoned".into())
        )?;

        let start_offset = state.current_offset;
        
        state.writer.write_all(buffer)?;
        state.current_offset += buffer.len() as u64;

        Ok(start_offset)
    }

    /// Flushes the buffer to disk.
    pub fn flush(&self) -> Result<()> {
        let mut state = self.inner.lock().map_err(|_| 
            crate::error::ParcodeError::Internal("SeqWriter Mutex poisoned".into())
        )?;
        state.writer.flush()?;
        Ok(())
    }

    /// Returns the current file cursor position.
    pub fn current_offset(&self) -> Result<u64> {
        let state = self.inner.lock().map_err(|_| 
            crate::error::ParcodeError::Internal("SeqWriter Mutex poisoned".into())
        )?;
        Ok(state.current_offset)
    }
}