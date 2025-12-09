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

    /// Serializes the object "shallowly" (only local fields, ignoring chunkable/map fields).
    /// This is used when the object is part of a container (like Vec) that forms a chunk.
    fn serialize_shallow<W: std::io::Write>(&self, writer: &mut W) -> crate::error::Result<()>
    where
        Self: serde::Serialize,
    {
        // Default: serialize everything
        crate::internal::bincode::serde::encode_into_std_write(
            self,
            writer,
            crate::internal::bincode::config::standard(),
        )
        .map_err(|e| crate::error::ParcodeError::Serialization(e.to_string()))
        .map(|_| ())
    }

    /// Serializes a slice of objects.
    /// Default implementation uses bincode for the whole slice (optimized).
    /// Types with chunkable fields should override this to iterate and call serialize_shallow.
    fn serialize_slice<W: std::io::Write>(
        slice: &[Self],
        writer: &mut W,
    ) -> crate::error::Result<()>
    where
        Self: Sized + serde::Serialize,
    {
        crate::internal::bincode::serde::encode_into_std_write(
            slice,
            writer,
            crate::internal::bincode::config::standard(),
        )
        .map_err(|e| crate::error::ParcodeError::Serialization(e.to_string()))
        .map(|_| ())
    }

    /// Visits the object as an "inlined" item within a container (e.g., `Vec`).
    ///
    /// In this mode, the object does NOT create a new node for itself, as its
    /// "local" data is already serialized in the container's payload.
    /// However, it must still visit its "remote" (chunkable) children and link
    /// them to the container's node (`parent_id`).
    fn visit_inlined<'a>(
        &'a self,
        _graph: &mut TaskGraph<'a>,
        _parent_id: ChunkId,
        _config_override: Option<JobConfig>,
    ) {
        // Default: Do nothing (no children to visit or link).
        // Override this for structs with #[parcode(chunkable)] fields.
    }
}
