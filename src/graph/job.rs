use crate::error::Result;
use crate::format::ChildRef;

/// Execution configuration for a specific node.
/// Kept small (1 byte) for efficient pass-by-copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobConfig {
    /// Compression algorithm ID.
    /// 0 = No Compression (Default)
    /// 1 = Lz4 (if feature enabled)
    /// 2..255 = Reserved
    pub compression_id: u8,
}

impl Default for JobConfig {
    fn default() -> Self {
        Self { compression_id: 0 }
    }
}

/// Represents a unit of work: a piece of data that knows how to serialize itself.
///
/// # Lifetimes
/// * `'a`: The lifetime of the data being serialized. This allows the Job to hold
///   references (e.g., `&'a [u8]`) instead of owning the data, enabling Zero-Copy writes.
pub trait SerializationJob<'a>: Send + Sync {
    /// Executes the serialization logic, producing a raw byte buffer.
    fn execute(&self, children_refs: &[ChildRef]) -> Result<Vec<u8>>;

    /// Returns an estimated size in bytes for scheduling heuristics.
    fn estimated_size(&self) -> usize;

    /// Returns the specific configuration for this job.
    fn config(&self) -> JobConfig {
        JobConfig::default()
    }
}

impl<'a> std::fmt::Debug for Box<dyn SerializationJob<'a> + 'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SerializationJob(size={}, algo={})",
            self.estimated_size(),
            self.config().compression_id
        )
    }
}
