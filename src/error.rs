//! Centralized error handling for Parcode.
//!
//! This module provides a robust error handling system that strictly avoids panics,
//! ensuring that all failure conditions are properly propagated through the `Result` type.
//!
//! ## Design Philosophy
//!
//! Parcode's error handling is designed with the following principles:
//!
//! 1. **No Panics:** All error conditions are represented as `Result` values. The library
//!    enforces this through `#![deny(clippy::panic)]` and `#![deny(clippy::unwrap_used)]`.
//!
//! 2. **Contextual Information:** Errors include descriptive messages that help diagnose
//!    the root cause of failures.
//!
//! 3. **Error Chaining:** Where possible, errors preserve the underlying cause through
//!    the `source()` method, enabling full error traces.
//!
//! 4. **Cloneable Errors:** The [`ParcodeError`] type is `Clone`, allowing errors to be
//!    shared across threads or stored for later analysis.
//!
//! ## Error Categories
//!
//! Errors are categorized by their domain:
//!
//! - **I/O Errors** ([`ParcodeError::Io`]): Low-level file system operations
//! - **Serialization Errors** ([`ParcodeError::Serialization`]): Bincode encoding/decoding
//! - **Compression Errors** ([`ParcodeError::Compression`]): Compression/decompression failures
//! - **Format Errors** ([`ParcodeError::Format`]): Invalid file format or corruption
//! - **Internal Errors** ([`ParcodeError::Internal`]): Logic errors (should not occur in production)
//!
//! ## Usage Patterns
//!
//! ### Basic Error Handling
//!
//! ```rust
//! use parcode::{Parcode, ParcodeError, ParcodeObject};
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Serialize, Deserialize, ParcodeObject)]
//! struct MyData { val: i32 }
//! let my_data = MyData { val: 10 };
//!
//! match Parcode::save("data_err.par", &my_data) {
//!     Ok(()) => println!("Saved successfully"),
//!     Err(ParcodeError::Io(e)) => eprintln!("I/O error: {}", e),
//!     Err(e) => eprintln!("Other error: {}", e),
//! }
//! # std::fs::remove_file("data_err.par").ok();
//! ```
//!
//! ### Error Propagation with `?`
//!
//! ```rust
//! use parcode::{Parcode, ParcodeObject};
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Serialize, Deserialize, ParcodeObject)]
//! struct GameState { level: u32 }
//!
//! fn save_game_state(state: &GameState) -> parcode::Result<()> {
//!     Parcode::save("game_err.par", state)?;
//!     Ok(())
//! }
//! # let state = GameState { level: 1 };
//! # save_game_state(&state)?;
//! # std::fs::remove_file("game_err.par").ok();
//! # Ok::<(), parcode::ParcodeError>(())
//! ```
//!
//! ### Accessing Error Sources
//!
//! ```rust
//! use std::error::Error;
//! use parcode::{Parcode, ParcodeObject};
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Serialize, Deserialize, ParcodeObject)]
//! struct MyData { val: i32 }
//! let my_data = MyData { val: 10 };
//!
//! if let Err(e) = Parcode::save("data_source.par", &my_data) {
//!     eprintln!("Error: {}", e);
//!     if let Some(source) = e.source() {
//!         eprintln!("Caused by: {}", source);
//!     }
//! }
//! # std::fs::remove_file("data_source.par").ok();
//! ```

use std::fmt;
use std::io;
use std::sync::Arc;

/// A specialized `Result` type for Parcode operations.
///
/// This type alias is used throughout the library to simplify error handling.
/// It is equivalent to `std::result::Result<T, ParcodeError>`.
///
/// ## Examples
///
/// ```rust
/// use parcode::Result;
///
/// fn my_function() -> Result<i32> {
///     Ok(42)
/// }
/// ```
pub type Result<T> = std::result::Result<T, ParcodeError>;

/// The master error enum covering all failure domains in Parcode.
///
/// This enum represents all possible error conditions that can occur during
/// Parcode operations. Each variant corresponds to a specific failure domain
/// and contains contextual information about the error.
///
/// ## Variants
///
/// - **Io:** Low-level I/O failures (file not found, permission denied, disk full, etc.)
/// - **Serialization:** Bincode encoding/decoding failures (type mismatch, invalid data, etc.)
/// - **Compression:** Compression algorithm failures (corrupted compressed data, etc.)
/// - **Format:** File format validation failures (wrong magic bytes, version mismatch, corruption)
/// - **Internal:** Logic errors in the library (should not occur in production; please report as bugs)
///
/// ## Cloneability
///
/// This type is `Clone` to support error sharing across threads and storage for later analysis.
/// I/O errors are wrapped in `Arc` to make cloning efficient.
///
/// ## Examples
///
/// ```rust
/// use parcode::ParcodeError;
///
/// fn check_error(err: &ParcodeError) {
///     match err {
///         ParcodeError::Io(e) => println!("I/O error: {}", e),
///         ParcodeError::Format(msg) => println!("Format error: {}", msg),
///         _ => println!("Other error"),
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub enum ParcodeError {
    /// Low-level I/O failure (disk full, permissions, network errors, etc.).
    ///
    /// The underlying `io::Error` is wrapped in an `Arc` to make the error `Clone`.
    /// This allows errors to be shared across threads without expensive copying.
    ///
    /// ## Common Causes
    ///
    /// - File not found
    /// - Permission denied
    /// - Disk full
    /// - Network interruption (for network file systems)
    /// - Hardware failure
    Io(Arc<io::Error>),

    /// Serialization or deserialization failure (bincode).
    ///
    /// This error occurs when bincode cannot encode or decode data, typically due to:
    ///
    /// - Type mismatches between serialized and deserialized types
    /// - Invalid or corrupted serialized data
    /// - Unsupported types or features
    ///
    /// The string contains a detailed error message from bincode.
    Serialization(String),

    /// Compression algorithm failure.
    ///
    /// This error occurs when compression or decompression fails, typically due to:
    ///
    /// - Corrupted compressed data
    /// - Unsupported compression algorithm ID
    /// - Compression buffer overflow
    ///
    /// The string contains a detailed error message from the compression library.
    Compression(String),

    /// The file format is invalid, corrupted, or has a version mismatch.
    ///
    /// This error occurs when the file does not conform to the Parcode V4 format:
    ///
    /// - Wrong magic bytes (not "PAR4")
    /// - Unsupported version number
    /// - Truncated file (missing header or chunks)
    /// - Corrupted chunk metadata
    /// - Invalid child references
    ///
    /// The string contains a detailed description of the format violation.
    Format(String),

    /// Logic error in the graph scheduler or other internal components.
    ///
    /// This error should not occur in production. If you encounter this error, it likely
    /// indicates a bug in the library. Please report it with a minimal reproduction case.
    ///
    /// ## Common Causes (all indicate bugs)
    ///
    /// - Cyclic dependencies in the graph
    /// - Mutex poisoning
    /// - Invalid node IDs
    /// - Missing child results
    ///
    /// The string contains diagnostic information about the internal error.
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
