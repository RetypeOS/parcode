//! Defines the `ParcodeVisitor` trait for graph construction.
//!
//! This module allows types to define how they should be split into nodes
//! in the `TaskGraph`.

//use crate::graph::core::TaskGraph;
//use crate::graph::id::ChunkId;
//use crate::graph::job::SerializationJob;
use crate::graph::{ChunkId, JobConfig, SerializationJob, TaskGraph};

/// A trait for types that can be structurally visited to build a Parcode `TaskGraph`.
///
/// This is distinct from `serde::Serialize`. Instead of writing bytes,
/// this trait writes *Nodes* into the graph.
pub trait ParcodeVisitor {
    /// Visits the object and populates the graph.
    ///
    /// # Lifetimes
    /// * `'a`: The lifetime of the graph. `Self` must outlive `'a` if we want to
    ///   store references to `self` inside the graph (Zero-Copy).
    fn visit<'a>(
        &'a self,
        graph: &mut TaskGraph<'a>,
        parent_id: Option<ChunkId>,
        config_override: Option<JobConfig>,
    );

    /// Helper method to create the specific Job for this type.
    fn create_job<'a>(
        &'a self,
        config_override: Option<JobConfig>,
    ) -> Box<dyn SerializationJob<'a> + 'a>;
}

// Example implementation for primitives (leaves in the graph logic,
// usually inlined, but here shown if they were chunks).
// In reality, primitives are almost always part of a parent chunk payload.
