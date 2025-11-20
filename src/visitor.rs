//! PLACEHOLDER

//use crate::graph::core::TaskGraph;
//use crate::graph::id::ChunkId;
//use crate::graph::job::SerializationJob;
use crate::graph::{TaskGraph, ChunkId, SerializationJob, JobConfig};

/// A trait for types that can be structurally visited to build a Parcode TaskGraph.
///
/// This is distinct from `serde::Serialize`. Instead of writing bytes,
/// this trait writes *Nodes* into the graph.
pub trait ParcodeVisitor {
    /// Visits the object and populates the graph.
    /// 
    /// # Arguments
    /// * `config_override`: If `Some`, implies that the parent requested a specific configuration 
    ///   (e.g., compression) for this node. Implementations MUST apply this to the created job.
    fn visit(&self, graph: &mut TaskGraph, parent_id: Option<ChunkId>, config_override: Option<JobConfig>);
    
    /// Helper method to create the specific Job for this type.
    fn create_job(&self, config_override: Option<JobConfig>) -> Box<dyn SerializationJob>;
}

// Example implementation for primitives (leaves in the graph logic,
// usually inlined, but here shown if they were chunks).
// In reality, primitives are almost always part of a parent chunk payload.
