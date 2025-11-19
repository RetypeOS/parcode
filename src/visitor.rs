use crate::graph::core::TaskGraph;
use crate::graph::id::ChunkId;
use crate::graph::job::SerializationJob;

/// A trait for types that can be structurally visited to build a Parcode TaskGraph.
///
/// This is distinct from `serde::Serialize`. Instead of writing bytes, 
/// this trait writes *Nodes* into the graph.
pub trait ParcodeVisitor {
    /// Visits the object and populates the graph.
    ///
    /// # logical Flow
    /// 1. Create a `SerializationJob` for *this* object.
    /// 2. Add it to the graph -> get `my_id`.
    /// 3. If this object has children marked as `#[chunkable]`, recurse:
    ///    - `child.visit(graph, Some(my_id))`
    /// 4. If `parent_id` is provided, link `parent_id -> my_id`.
    fn visit(&self, graph: &mut TaskGraph, parent_id: Option<ChunkId>);
    
    /// Helper method to create the specific Job for this type.
    /// This allows decoupling the visiting logic from the serialization logic.
    fn create_job(&self) -> Box<dyn SerializationJob>;
}

// Example implementation for primitives (leaves in the graph logic, 
// usually inlined, but here shown if they were chunks).
// In reality, primitives are almost always part of a parent chunk payload.