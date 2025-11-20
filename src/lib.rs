//! # Parcode V3
//! 
//! A high-performance, graph-based serialization library for Rust.

#![deny(unsafe_code)] 
#![deny(clippy::unwrap_used)]
#![deny(clippy::panic)]
#![warn(missing_docs)]

pub mod compression;
pub mod error;
pub mod format;
pub mod io;
pub mod visitor;
pub mod graph;
pub mod executor;
pub mod api;
pub mod reader;

mod visitor_impls; 

// --- MÓDULOS DE SOPORTE PARA LA MACRO ---

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

pub use compression::{Compressor, NoCompression};
#[cfg(feature = "lz4_flex")]
pub use compression::Lz4Compressor;

pub use error::{ParcodeError, Result};
pub use api::Parcode;
pub use reader::ParcodeReader;

// --- LA PIEZA FALTANTE ---
// Re-exportamos la macro derive para que sea accesible como `parcode::ParcodeObject`
pub use parcode_derive::ParcodeObject;

///PLACEHOLDER
pub mod constants {
    /// PLACEHOLDER
    pub const DEFAULT_BUFFER_SIZE: usize = 8 * 1024;
}