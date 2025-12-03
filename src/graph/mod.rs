//! Core graph definitions for the Parcode execution engine.
//!
//! This module defines the `TaskGraph`, `Node`, and `SerializationJob` structures
//! that form the backbone of the parallel serialization process.

/// Defines the `TaskGraph` and `Node` structures.
pub mod core;
/// Defines the `ChunkId` type.
pub mod id;
/// Defines the `SerializationJob` trait and `JobConfig`.
pub mod job;

pub use core::{Node, TaskGraph};
pub use id::ChunkId;
pub use job::{JobConfig, SerializationJob};
