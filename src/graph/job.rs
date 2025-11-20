use crate::error::Result;
use crate::format::ChildRef;
use std::any::Any;

/// Represents a unit of work: a piece of data that knows how to serialize itself
/// once its dependencies (children) have been written.
pub trait SerializationJob: Send + Sync {
    /// Serializes the payload into bytes.
    ///
    /// # Arguments
    /// * `children_refs`: A list of references (offset/length) for the children chunks
    ///   that this node depends on. These must be embedded into the serialized data
    ///   (e.g., in the Footer).
    fn execute(&self, children_refs: &[ChildRef]) -> Result<Vec<u8>>;

    /// Returns the estimated size in bytes (heuristic).
    /// Used for the "Inlining" optimization pass.
    fn estimated_size(&self) -> usize;

    /// Helper for debugging.
    fn as_any(&self) -> &dyn Any;
}

impl std::fmt::Debug for Box<dyn SerializationJob> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SerializationJob")
    }
}
