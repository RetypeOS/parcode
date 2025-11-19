// src/lib.rs

//! # Parcode V2
//! 
//! A high-performance, graph-based serialization library for Rust.

// Modificamos la política de unsafe para permitir excepciones locales con #[allow]
#![deny(unsafe_code)] 
#![deny(clippy::unwrap_used)]
#![deny(clippy::panic)]
#![warn(missing_docs)]

pub mod compression;
pub mod error;
pub mod format;
pub mod io;

/// Module for visitor pattern interfaces.
pub mod visitor;
/// Module for dependency graph definitions.
pub mod graph;
/// Module for the parallel execution engine.
pub mod executor;
/// High-level public API.
pub mod api;
/// Reader implementation.
pub mod reader;

// Incluimos las implementaciones de colecciones (Vec, etc)
mod visitor_impls; 

// Re-exports
pub use compression::{Compressor, NoCompression};
#[cfg(feature = "lz4_flex")]
pub use compression::Lz4Compressor;
pub use error::{ParcodeError, Result};
pub use api::Parcode;
pub use reader::ParcodeReader;

// Derive macro
// pub use parcode_derive::ParcodeObject;

/// PLACEHOLDER
pub mod constants {
    /// PLACEHOLDER
    pub const DEFAULT_BUFFER_SIZE: usize = 8 * 1024;
}