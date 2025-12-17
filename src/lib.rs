//! # Parcode V4
//!
//! A high-performance, graph-based serialization library for Rust that enables lazy loading,
//! zero-copy operations, and parallel processing of complex data structures.
//!
//! ## Overview
//!
//! Parcode is fundamentally different from traditional serialization libraries. Instead of treating
//! data as a monolithic blob, Parcode analyzes the structural relationships within your data and
//! creates a dependency graph where each node represents a serializable chunk. This architectural
//! approach enables several powerful capabilities:
//!
//! ### Key Features
//!
//! *   **Parallel Serialization:** Independent chunks are serialized and compressed concurrently
//!     using Rayon's work-stealing scheduler, maximizing CPU utilization.
//! *   **Parallel Deserialization:** Reading operations leverage memory mapping and parallel
//!     reconstruction to minimize I/O latency and maximize throughput.
//! *   **Zero-Copy Operations:** Data is read directly from memory-mapped files where possible,
//!     eliminating unnecessary allocations and copies.
//! *   **Lazy Loading:** Navigate deep object hierarchies without loading data from disk. Only
//!     the metadata is read initially, and actual data is loaded on-demand.
//! *   **Surgical Access:** Load only specific fields, vector elements, or map entries without
//!     deserializing the entire structure.
//! *   **O(1) Map Lookups:** Hash-based sharding enables constant-time lookups in large `HashMaps`
//!     without loading the entire collection.
//! *   **Streaming/Partial Reads:** Large collections can be iterated over without loading the
//!     entire dataset into RAM.
//!
//! ## Architecture
//!
//! ### The Graph Model
//!
//! When you serialize an object with Parcode, the library constructs a directed acyclic graph (DAG)
//! where:
//! - Each node represents a serializable chunk of data
//! - Edges represent dependencies (parent-child relationships)
//! - Leaf nodes contain primitive data or small collections
//! - Internal nodes contain metadata and references to their children
//!
//! This graph is then executed bottom-up: leaves are processed first, and parents are processed
//! only after all their children have completed. This enables maximum parallelism while maintaining
//! data integrity.
//!
//! ### File Format (V4)
//!
//! The physical layout on disk follows a "children-first" ordering:
//! ```text
//! [Leaf Chunk 1] [Leaf Chunk 2] ... [Parent Chunk] [Root Chunk] [Global Header]
//! ```
//!
//! Each chunk is self-contained with:
//! ```text
//! [Compressed Payload] [Children Table (Optional)] [MetaByte]
//! ```
//!
//! The Global Header at the end of the file points to the Root Chunk, which serves as the
//! entry point for reading operations.
//!
//! ## Core Concepts
//!
//! ### `TaskGraph`
//!
//! The [`graph::TaskGraph`] is the central structure representing the object graph to be serialized.
//! It acts as an arena allocator for nodes and manages the dependency relationships between them.
//! The graph lifetime is tied to the input data, enabling zero-copy serialization.
//!
//! ### Executor
//!
//! The [`executor`] module contains the engine that drives parallel execution of the graph.
//! It orchestrates the serialization → compression → I/O pipeline, using atomic operations
//! for lock-free dependency tracking and Rayon for work distribution.
//!
//! ### Reader
//!
//! The [`ParcodeReader`] is responsible for memory-mapping the file and reconstructing objects.
//! It provides both eager (full deserialization) and lazy (on-demand) reading strategies,
//! automatically selecting the optimal approach based on the data type.
//!
//! ### Visitor
//!
//! The [`visitor::ParcodeVisitor`] trait allows types to define how they should be decomposed
//! into graph nodes. The `#[derive(ParcodeObject)]` macro automatically implements this trait,
//! analyzing field attributes to determine which fields should be inlined vs. stored as
//! separate chunks.
//!
//! ## Usage Patterns
//!
//! ### Basic Serialization
//!
//! ```rust,ignore
//! use parcode::{Parcode, ParcodeObject};
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Serialize, Deserialize, ParcodeObject)]
//! struct GameState {
//!     level: u32,
//!     score: u64,
//!     #[parcode(chunkable)]
//!     player_data: PlayerData,
//! }
//!
//! // Save
//! let state = GameState { /* ... */ };
//! Parcode::save("game.par", &state)?;
//!
//! // Load (eager)
//! let loaded: GameState = Parcode::read("game.par")?;
//! ```
//!
//! ### Lazy Loading
//!
//! ```rust,ignore
//! use parcode::ParcodeReader;
//!
//! let reader = ParcodeReader::open("game.par")?;
//! let state_lazy = reader.read_lazy::<GameState>()?;
//!
//! // Access local fields instantly (already in memory)
//! println!("Level: {}", state_lazy.level);
//!
//! // Load remote fields on-demand
//! let player = state_lazy.player_data.load()?;
//! ```
//!
//! ### `HashMap` Sharding
//!
//! ```rust,ignore
//! #[derive(Serialize, Deserialize, ParcodeObject)]
//! struct Database {
//!     #[parcode(map)]  // Enable O(1) lookups
//!     users: HashMap<u64, User>,
//! }
//!
//! let db_lazy = reader.read_lazy::<Database>()?;
//! let user = db_lazy.users.get(&12345)?.expect("User not found");
//! ```
//!
//! ## Performance Considerations
//!
//! - **Write Performance:** Scales linearly with CPU cores for independent chunks
//! - **Read Performance:** Memory-mapped I/O provides near-instant startup times
//! - **Memory Usage:** Lazy loading keeps memory footprint minimal
//! - **Compression:** Optional LZ4 compression (feature: `lz4_flex`) trades CPU for I/O bandwidth
//!
//! ### Safety and Error Handling
//!
//! Parcode is designed with safety as a priority:
//!
//! * **Encapsulated Unsafe:** `unsafe` code is used sparingly and only in the `reader` module
//!   to achieve zero-copy parallel stitching of vectors. These sections are strictly auditied.
//! * **No Panics:** No `unwrap()` or `panic!()` calls in the library (enforced by clippy lints).
//! * **Comprehensive Errors:** All failures correspond to a [`ParcodeError`] type.
//! * **Robust I/O:** Mutex poisoning and partial writes are handled gracefully.

#![deny(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::panic)]
#![warn(missing_docs)]

// --- PUBLIC API MODULES ---
pub mod api;
pub mod compression;
pub mod error;
pub mod format;
pub mod inspector;
pub mod reader;
pub mod visitor;

// --- INTERNAL IMPLEMENTATION MODULES (Hidden from Docs) ---
#[doc(hidden)]
pub mod executor;
#[doc(hidden)]
pub mod graph;
#[doc(hidden)]
pub mod io;
#[doc(hidden)]
pub mod map;

// Private modules
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
