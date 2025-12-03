//! # Parcode V3
//!
//! A high-performance, graph-based serialization library for Rust.
//!
//! Parcode is designed to handle large, complex data structures by breaking them down into
//! a dependency graph of chunks. This allows for:
//!
//! *   **Parallel Serialization:** Independent chunks are serialized and compressed concurrently.
//! *   **Parallel Deserialization:** Reading utilizes memory mapping and parallel reconstruction.
//! *   **Zero-Copy Operations:** Where possible, data is read directly from the memory-mapped file.
//! *   **Streaming/Partial Reads:** Large collections can be iterated over without loading the entire dataset into RAM.
//!
//! ## Core Concepts
//!
//! *   **`TaskGraph`:** The central structure representing the object graph to be serialized.
//! *   **`Executor`:** The engine that drives the parallel execution of the graph (serialization -> compression -> I/O).
//! *   **`Reader`:** The component responsible for mapping the file and reconstructing objects.
//! *   **`Visitor`:** The trait that allows types to define how they should be split into graph nodes.

#![deny(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::panic)]
#![warn(missing_docs)]

pub mod api;
pub mod compression;
pub mod error;
pub mod executor;
pub mod format;
pub mod graph;
pub mod io;
pub mod reader;
pub mod visitor;

mod visitor_impls;

// --- MACRO SUPPORT MODULES ---

/// Runtime utilities used by the derived code.
#[doc(hidden)]
pub mod rt;

/// Internal re-exports for the macro to ensure dependencies are available.
#[doc(hidden)]
pub mod internal {
    pub use bincode;
    pub use serde;
}

// --- RE-EXPORTS ---

#[cfg(feature = "lz4_flex")]
pub use compression::Lz4Compressor;
pub use compression::{Compressor, NoCompression};

pub use api::Parcode;
pub use error::{ParcodeError, Result};
pub use reader::ParcodeReader;

// Re-export the derive macro so it is accessible as `parcode::ParcodeObject`
pub use parcode_derive::ParcodeObject;

/// Constants used throughout the library.
pub mod constants {
    /// The default buffer size for I/O operations.
    pub const DEFAULT_BUFFER_SIZE: usize = 8 * 1024;
}
