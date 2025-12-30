use crate::error::Result;
use crate::format::ChildRef;

/// Execution configuration for a specific node.
/// Kept small (1 byte) for efficient pass-by-copy.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobConfig {
    /// Compression algorithm ID.
    /// 0 = No Compression (Default)
    /// 1 = Lz4 (if feature enabled)
    /// 2..255 = Reserved
    pub compression_id: u8,
    /// Whether to use the optimized Map Sharding strategy.
    pub is_map: bool,
}

/// Represents a unit of work: a piece of data that knows how to serialize itself.
///
/// This trait is the primary abstraction for the concurrent execution engine. Each node
/// in the [`TaskGraph`](crate::graph::TaskGraph) holds a `SerializationJob`.
///
/// # Lifetimes
///
/// * `'a`: The lifetime of the data being serialized. This allows the Job to hold
///   references (e.g., `&'a [u8]`) instead of owning the data, enabling Zero-Copy writes.
///
/// # Thread Safety
///
/// Implementations must be `Send + Sync` to support parallel execution across threads.
pub trait SerializationJob<'a>: Send + Sync {
    /// Executes the serialization logic, producing a raw byte buffer.
    ///
    /// This method is called by the executor when all dependencies of the node have
    /// completed.
    ///
    /// ## Parameters
    ///
    /// * `children_refs`: A slice containing the offsets and lengths of all child chunks
    ///   in the final file. These can be used to build a footer or index for the chunk.
    ///
    /// ## Returns
    ///
    /// Returns a `Vec<u8>` containing the serialized payload.
    fn execute(&self, children_refs: &[ChildRef]) -> Result<Vec<u8>>;

    /// Returns an estimated size in bytes for scheduling heuristics.
    ///
    /// The executor uses this value to decide which jobs to run first or how to
    /// distribute work across cores. An accurate estimate improves performance.
    fn estimated_size(&self) -> usize;

    /// Returns the specific configuration for this job.
    ///
    /// This includes settings like compression algorithm or sharding strategy.
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
