//! Centralized error handling for Parcode.
//!
//! This module strictly avoids panics, providing a robust `Result` type
//! that propagates context up the stack.

use std::fmt;
use std::io;
use std::sync::Arc;

/// A specialized Result type for Parcode operations.
pub type Result<T> = std::result::Result<T, ParcodeError>;

/// The master error enum covering all failure domains.
#[derive(Debug, Clone)]
pub enum ParcodeError {
    /// Low-level I/O failure (disk full, permissions, etc.).
    Io(Arc<io::Error>),

    /// Serialization/Deserialization failure (bincode).
    Serialization(String),

    /// Compression algorithm failure.
    Compression(String),

    /// The file format is invalid, corrupted, or version mismatch.
    Format(String),

    /// Logic error in the graph scheduler (should not happen in prod).
    Internal(String),
}

impl fmt::Display for ParcodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O Error: {e}"),
            Self::Serialization(s) => write!(f, "Serialization Error: {s}"),
            Self::Compression(s) => write!(f, "Compression Error: {s}"),
            Self::Format(s) => write!(f, "Format Error: {s}"),
            Self::Internal(s) => write!(f, "Internal Logic Error: {s}"),
        }
    }
}

impl std::error::Error for ParcodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for ParcodeError {
    fn from(err: io::Error) -> Self {
        Self::Io(Arc::new(err))
    }
}
