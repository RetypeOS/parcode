use std::fmt;

/// A strong type representing a unique identifier for a node in the task graph.
/// This maps 1:1 to a future chunk on disk.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChunkId(u32); // u32 is sufficient for 4 billion chunks per file.

impl ChunkId {
    /// Creates a new ChunkId.
    /// Restrict visibility to the graph module to prevent arbitrary creation.
    pub(crate) fn new(id: u32) -> Self {
        Self(id)
    }

    /// Returns the raw numeric value.
    pub fn as_u32(&self) -> u32 {
        self.0
    }
}

impl fmt::Debug for ChunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChunkId({})", self.0)
    }
}

impl fmt::Display for ChunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}