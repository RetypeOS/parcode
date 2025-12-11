//! High-level API for Parcode.
//!
//! This module provides the primary entry points for reading and writing Parcode files.
//! It offers a builder-style interface for configuration and convenience methods for
//! common operations.
//!
//! ## Design Philosophy
//!
//! The API is designed around two core principles:
//!
//! 1. **Simplicity for Common Cases:** The [`Parcode::save`] and [`Parcode::read`] methods
//!    provide zero-configuration serialization for most use cases.
//!
//! 2. **Flexibility for Advanced Use:** The builder pattern ([`Parcode::builder`]) allows
//!    fine-grained control over compression, buffer sizes, and other parameters.
//!
//! ## Usage Patterns
//!
//! ### Quick Start (Default Settings)
//!
//! ```rust,ignore
//! use parcode::Parcode;
//!
//! // Serialize with default settings (no compression)
//! Parcode::save("data.par", &my_data)?;
//!
//! // Deserialize
//! let loaded: MyType = Parcode::read("data.par")?;
//! ```
//!
//! ### Custom Configuration
//!
//! ```rust,ignore
//! use parcode::Parcode;
//!
//! // Enable compression
//! Parcode::builder()
//!     .compression(true)
//!     .write("data.par", &my_data)?;
//! ```
//!
//! ## Thread Safety
//!
//! All methods in this module are thread-safe. The serialization process uses interior
//! mutability where necessary (e.g., in the writer) and relies on Rayon for parallel
//! execution, which handles synchronization automatically.

use crate::error::Result;
use crate::executor::execute_graph;
use crate::format::GlobalHeader;
use crate::graph::TaskGraph;
use crate::io::SeqWriter;
use crate::reader::{ParcodeNative, ParcodeReader};
use crate::visitor::ParcodeVisitor;
use std::path::Path;

/// The main entry point for configuring and executing Parcode operations.
///
/// This struct provides a builder-style API for configuring serialization parameters
/// and executing read/write operations. It is designed to be lightweight and can be
/// constructed multiple times without significant overhead.
///
/// ## Configuration Options
///
/// - **Compression:** Enable or disable compression (default: disabled). When enabled,
///   the library uses the default compression algorithm (LZ4 if the `lz4_flex` feature
///   is enabled, otherwise no compression).
///
/// ## Examples
///
/// ### Basic Usage
///
/// ```rust,ignore
/// use parcode::Parcode;
///
/// let data = vec![1, 2, 3];
/// Parcode::save("data.par", &data).unwrap();
/// let loaded: Vec<i32> = Parcode::read("data.par").unwrap();
/// ```
///
/// ### With Compression
///
/// ```rust,ignore
/// use parcode::Parcode;
///
/// let data = vec![1, 2, 3];
/// Parcode::builder()
///     .compression(true)
///     .write("data.par", &data)?;
/// ```
///
/// ## Performance Notes
///
/// - The builder itself has negligible overhead (it's just a small struct with flags)
/// - Compression trades CPU time for reduced I/O and storage
/// - Parallel execution scales with the number of independent chunks in your data
#[derive(Debug, Default)]
pub struct Parcode {
    /// Whether to enable compression for serialized chunks.
    ///
    /// When `true`, the default compression algorithm will be used. The actual algorithm
    /// depends on enabled features (LZ4 if `lz4_flex` is enabled).
    use_compression: bool,
}

impl Parcode {
    /// Creates a new `Parcode` builder with default settings.
    ///
    /// This is the entry point for the builder pattern. The returned instance can be
    /// configured using method chaining before calling [`write`](Self::write).
    ///
    /// ## Default Settings
    ///
    /// - Compression: Disabled
    ///
    /// ## Examples
    ///
    /// ```rust,ignore
    /// use parcode::Parcode;
    ///
    /// let parcode = Parcode::builder()
    ///     .compression(true);
    /// ```
    ///
    /// ## Performance
    ///
    /// This method is extremely lightweight (just creates a small struct) and can be
    /// called repeatedly without concern for performance.
    pub fn builder() -> Self {
        Self::default()
    }

    /// Enables or disables compression for all chunks.
    ///
    /// When compression is enabled, the library will use the default compression algorithm
    /// to reduce the size of serialized chunks. The actual algorithm used depends on which
    /// features are enabled:
    ///
    /// - If the `lz4_flex` feature is enabled: LZ4 compression (fast, moderate ratio)
    /// - Otherwise: No compression (pass-through)
    ///
    /// ## Parameters
    ///
    /// - `enable`: Whether to enable compression
    ///
    /// ## Returns
    ///
    /// Returns `self` to allow method chaining.
    ///
    /// ## Trade-offs
    ///
    /// - **Enabled:** Smaller files, lower I/O bandwidth, higher CPU usage
    /// - **Disabled:** Larger files, higher I/O bandwidth, lower CPU usage
    ///
    /// ## Examples
    ///
    /// ```rust,ignore
    /// use parcode::Parcode;
    ///
    /// // Enable compression
    /// Parcode::builder()
    ///     .compression(true)
    ///     .write("data.par", &my_data)?;
    /// ```
    ///
    /// ## Performance Notes
    ///
    /// LZ4 compression typically achieves 2-3x compression ratios on structured data
    /// with minimal CPU overhead. For data that doesn't compress well (e.g., already
    /// compressed images), the overhead may outweigh the benefits.
    pub fn compression(mut self, enable: bool) -> Self {
        self.use_compression = enable;
        self
    }

    /// Reads and fully deserializes an object from a Parcode file.
    ///
    /// This method automatically selects the optimal reconstruction strategy based on the
    /// type being read:
    ///
    /// - **Collections (`Vec<T>`):** Uses parallel reconstruction across shards
    /// - **Maps (`HashMap`<K, V>):** Reconstructs all shards and merges entries
    /// - **Primitives and Structs:** Uses sequential deserialization
    ///
    /// ## Type Parameters
    ///
    /// - `T`: The type to deserialize. Must implement [`ParcodeNative`].
    /// - `P`: The path type (anything that implements `AsRef<Path>`).
    ///
    /// ## Parameters
    ///
    /// - `path`: The file path to read from.
    ///
    /// ## Returns
    ///
    /// Returns the fully deserialized object of type `T`.
    ///
    /// ## Errors
    ///
    /// This method can fail if:
    ///
    /// - The file does not exist or cannot be opened
    /// - The file is not a valid Parcode file (wrong magic bytes or version)
    /// - The file is corrupted or truncated
    /// - Deserialization fails (e.g., type mismatch)
    /// - Decompression fails
    ///
    /// ## Examples
    ///
    /// ```rust,ignore
    /// use parcode::Parcode;
    ///
    /// // Read a vector
    /// let data: Vec<i32> = Parcode::read("numbers.par")?;
    ///
    /// // Read a custom struct
    /// let state: GameState = Parcode::read("game.par")?;
    /// ```
    ///
    /// ## Performance
    ///
    /// - **Memory Mapping:** The file is memory-mapped, so the OS handles paging
    /// - **Parallel Reconstruction:** Collections are reconstructed in parallel
    /// - **Memory Usage:** The entire object is loaded into memory
    ///
    /// For large files where you don't need the entire object, consider using
    /// [`ParcodeReader::read_lazy`](crate::ParcodeReader::read_lazy) instead.
    pub fn read<T, P>(path: P) -> Result<T>
    where
        T: ParcodeNative,
        P: AsRef<Path>,
    {
        let reader = ParcodeReader::open(path)?;
        let root = reader.root()?;

        // Dispatches to Vec::from_node (Parallel) or T::from_node (Simple)
        T::from_node(&root)
    }

    /// Saves an object to a file using default settings (no compression).
    ///
    /// This is a convenience method that creates a default `Parcode` instance and calls
    /// [`write`](Self::write). It's equivalent to:
    ///
    /// ```rust,ignore
    /// Parcode::default().write(path, root_object)
    /// ```
    ///
    /// ## Type Parameters
    ///
    /// - `T`: The type to serialize. Must implement [`ParcodeVisitor`]
    ///   and `Sync` (for parallel execution).
    /// - `P`: The path type (anything that implements `AsRef<Path>`).
    ///
    /// ## Parameters
    ///
    /// - `path`: The file path to write to. If the file exists, it will be truncated.
    /// - `root_object`: A reference to the object to serialize.
    ///
    /// ## Returns
    ///
    /// Returns `Ok(())` on success.
    ///
    /// ## Errors
    ///
    /// This method can fail if:
    ///
    /// - The file cannot be created (e.g., permission denied, disk full)
    /// - Serialization fails (e.g., bincode error)
    /// - I/O errors occur during writing
    ///
    /// ## Examples
    ///
    /// ```rust,ignore
    /// use parcode::Parcode;
    /// use serde::{Serialize, Deserialize};
    ///
    /// #[derive(Serialize, Deserialize)]
    /// struct MyData {
    ///     value: i32,
    /// }
    ///
    /// let data = MyData { value: 42 };
    /// Parcode::save("data.par", &data)?;
    /// ```
    ///
    /// ## Performance
    ///
    /// - **Zero-Copy:** The serializer borrows data rather than cloning it
    /// - **Parallel Execution:** Independent chunks are processed concurrently
    /// - **Buffered I/O:** Uses a 16MB buffer to minimize syscalls
    ///
    /// For custom configuration (e.g., enabling compression), use the builder pattern:
    ///
    /// ```rust,ignore
    /// Parcode::builder()
    ///     .compression(true)
    ///     .write("data.par", &data)?;
    /// ```
    pub fn save<T, P>(path: P, root_object: &T) -> Result<()>
    where
        T: ParcodeVisitor + Sync,
        P: AsRef<Path>,
    {
        Self::default().write(path, root_object)
    }

    /// Serializes an object graph to disk with the configured settings.
    ///
    /// This method performs the complete serialization pipeline:
    ///
    /// 1. **Graph Construction:** Analyzes the object structure and builds a dependency graph
    /// 2. **Parallel Execution:** Processes independent chunks concurrently using Rayon
    /// 3. **Compression:** Applies compression to each chunk (if enabled)
    /// 4. **I/O:** Writes chunks to disk in a bottom-up order
    /// 5. **Header Writing:** Appends the global header pointing to the root chunk
    ///
    /// ## Type Parameters
    ///
    /// - `'a`: The lifetime of the object being serialized. The graph borrows from the object,
    ///   enabling zero-copy serialization.
    /// - `T`: The type to serialize. Must implement [`ParcodeVisitor`]
    ///   and `Sync` (for parallel execution).
    /// - `P`: The path type (anything that implements `AsRef<Path>`).
    ///
    /// ## Parameters
    ///
    /// - `path`: The file path to write to. If the file exists, it will be truncated.
    /// - `root_object`: A reference to the object to serialize. This reference must remain
    ///   valid for the entire duration of the serialization process.
    ///
    /// ## Returns
    ///
    /// Returns `Ok(())` on success.
    ///
    /// ## Errors
    ///
    /// This method can fail if:
    ///
    /// - The file cannot be created (e.g., permission denied, disk full)
    /// - Serialization fails (e.g., bincode error)
    /// - Compression fails
    /// - I/O errors occur during writing
    /// - The graph contains cycles (should not happen with valid `ParcodeVisitor` implementations)
    ///
    /// ## Examples
    ///
    /// ```rust,ignore
    /// use parcode::Parcode;
    ///
    /// let data = vec![1, 2, 3, 4, 5];
    ///
    /// // Write with compression
    /// Parcode::builder()
    ///     .compression(true)
    ///     .write("data.par", &data)?;
    /// ```
    ///
    /// ## Performance Characteristics
    ///
    /// - **Parallelism:** Scales with the number of independent chunks (typically O(cores))
    /// - **Memory:** Uses zero-copy where possible; peak memory is proportional to the
    ///   largest chunk plus buffer overhead
    /// - **I/O:** Buffered writes (16MB buffer) minimize syscalls
    /// - **Compression:** LZ4 compression adds ~10-20% CPU overhead but can reduce I/O by 2-3x
    ///
    /// ## Thread Safety
    ///
    /// This method is thread-safe. Multiple threads can call `write` concurrently on different
    /// `Parcode` instances without interference. The internal writer uses a mutex to serialize
    /// I/O operations.
    pub fn write<'a, T, P>(&self, path: P, root_object: &'a T) -> Result<()>
    where
        T: ParcodeVisitor + Sync,
        P: AsRef<Path>,
    {
        let path = path.as_ref();
        // The graph is tied to lifetime 'a of root_object
        let mut graph = TaskGraph::<'a>::new();

        root_object.visit(&mut graph, None, None);

        let writer = SeqWriter::create(path)?;
        let registry = crate::compression::CompressorRegistry::new();

        let root_child_ref = execute_graph(&graph, &writer, &registry)?;

        let header = GlobalHeader::new(root_child_ref.offset, root_child_ref.length);
        writer.write_all(&header.to_bytes())?;
        writer.flush()?;

        Ok(())
    }
}
