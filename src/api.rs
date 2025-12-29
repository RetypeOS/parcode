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
//! 1. **Simplicity for Common Cases:** The [`Parcode::save`] and [`Parcode::load`] methods
//!    provide zero-configuration serialization for most use cases.
//!
//! 2. **Flexibility for Advanced Use:** The builder pattern ([`Parcode::builder`] or [`ParcodeOptions::default`()]) allows
//!    fine-grained control over compression, buffer sizes, and other parameters.
//!
//! ## Usage Patterns
//!
//! ### Quick Start (Default Settings)
//!
//! ```rust
//! use parcode::{Parcode, ParcodeObject};
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Serialize, Deserialize, ParcodeObject, PartialEq, Debug)]
//! struct MyType { val: i32 }
//!
//! let my_data = MyType { val: 42 };
//!
//! // Serialize with default settings (no compression)
//! Parcode::save("data_quick.par", &my_data)?;
//!
//! // Deserialize
//! let loaded: MyType = Parcode::load("data_quick.par")?;
//! # std::fs::remove_file("data_quick.par")?;
//! # Ok::<(), parcode::ParcodeError>(())
//! ```
//!
//! ### Custom Configuration
//!
//! ```rust
//! use parcode::{Parcode, ParcodeObject};
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Serialize, Deserialize, ParcodeObject, PartialEq, Debug)]
//! struct MyType { val: i32 }
//!
//! let my_data = MyType { val: 42 };
//!
//! // Enable compression
//! Parcode::builder()
//!     .compression(true)
//!     .write("data_custom.par", &my_data)?;
//! # std::fs::remove_file("data_custom.par")?;
//! # Ok::<(), parcode::ParcodeError>(())
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
use crate::inspector::{DebugReport, ParcodeInspector};
use crate::io::SeqWriter;
use crate::reader::{ParcodeFile, ParcodeNative};
use crate::visitor::ParcodeVisitor;
use std::io::Write;
use std::path::Path;

/// The main entry point for Parcode.
///
/// Provides static methods for common operations (`save`, `load`, `open`).
#[derive(Debug)]
pub struct Parcode;

impl Parcode {
    /// Creates a new builder to configure serialization.
    pub fn builder() -> ParcodeOptions {
        ParcodeOptions::default()
    }

    /// Saves an object to a file with default settings.
    ///
    /// **Note:** This method is not available on WASM targets.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn save<T, P>(path: P, root_object: &T) -> Result<()>
    where
        T: ParcodeVisitor + Sync,
        P: AsRef<Path>,
    {
        ParcodeOptions::default().save(path, root_object)
    }

    /// PLACEHOLDER
    pub fn write<T, W>(writer: W, root_object: &T) -> Result<()>
    where
        T: ParcodeVisitor + Sync,
        W: Write + Send,
    {
        ParcodeOptions::default().write(writer, root_object)
    }

    /// PLACEHOLDER
    pub fn serialize<T>(root_object: &T) -> Result<Vec<u8>>
    where
        T: ParcodeVisitor + Sync,
    {
        let mut buffer = Vec::with_capacity(4096);
        Self::write(&mut buffer, root_object)?;
        Ok(buffer)
    }

    /// Loads an object fully into memory from a file.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load<T, P>(path: P) -> Result<T>
    where
        T: ParcodeNative,
        P: AsRef<Path>,
    {
        ParcodeFile::open(path)?.load()
    }

    /// PLACEHOLDER
    pub fn load_bytes<T: ParcodeNative>(data: Vec<u8>) -> Result<T> {
        ParcodeFile::from_bytes(data)?.load()
    }

    /// Opens a Parcode resource using the configured storage backend.
    ///
    /// # Arguments
    /// * `input`: Can be:
    ///   - `Vec<u8>`: Direct memory (WASM compatible).
    ///   - `Path / String`: File system path (Requires `mmap` or fallback read).
    ///
    /// # Example
    /// ```rust,ignore
    /// // Desktop (Mmap)
    /// Parcode::open("data.par")?;
    ///
    /// // WASM (Memory)
    /// let bytes = fetch(...).await;
    /// Parcode::open(bytes)?;
    /// ```
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open<P: AsRef<Path>>(path: P) -> Result<ParcodeFile> {
        ParcodeFile::open(path)
    }

    /// PLACEHOLDER
    pub fn open_bytes(data: Vec<u8>) -> Result<ParcodeFile> {
        ParcodeFile::from_bytes(data)
    }

    /// PLACEHOLDER
    pub fn write_sync<T, W>(writer: W, root_object: &T) -> Result<()>
    where
        T: ParcodeVisitor,
        W: Write + Send,
    {
        ParcodeOptions::default().write_sync(writer, root_object)
    }

    /// PLACEHOLDER
    #[cfg(not(target_arch = "wasm32"))]
    pub fn save_sync<T, P>(path: P, root_object: &T) -> Result<()>
    where
        T: ParcodeVisitor,
        P: AsRef<Path>,
    {
        ParcodeOptions::default().save_sync(path, root_object)
    }

    /// Generates a structural inspection report of a file without full initialization.
    ///
    /// This is a convenience wrapper equivalent to `ParcodeInspector::inspect`.
    ///
    /// # Example
    /// ```rust,ignore
    /// let report = Parcode::inspect("data.par")?;
    /// println!("{}", report);
    /// ```
    #[cfg(not(target_arch = "wasm32"))]
    pub fn inspect<P: AsRef<Path>>(path: P) -> Result<DebugReport> {
        ParcodeInspector::inspect(path)
    }

    /// PLACEHOLDER
    pub fn inspect_bytes(data: Vec<u8>) -> Result<DebugReport> {
        let file = ParcodeFile::from_bytes(data)?;
        ParcodeInspector::inspect_file(&file)
    }
}

/// Builder for configuring serialization options (compression, etc.).
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
/// ```rust
/// use parcode::Parcode;
///
/// let data = vec![1, 2, 3];
/// Parcode::save("data_basic.par", &data).unwrap();
/// let loaded: Vec<i32> = Parcode::load("data_basic.par").unwrap();
/// # std::fs::remove_file("data_basic.par").unwrap();
/// ```
///
/// ### With Compression
///
/// ```rust
/// use parcode::Parcode;
///
/// let data = vec![1, 2, 3];
/// Parcode::builder()
///     .compression(true)
///     .write("data_comp.par", &data)?;
/// # std::fs::remove_file("data_comp.par")?;
/// # Ok::<(), parcode::ParcodeError>(())
/// ```
///
/// ## Performance Notes
///
/// - The builder itself has negligible overhead (it's just a small struct with flags)
/// - Compression trades CPU time for reduced I/O and storage
/// - Parallel execution scales with the number of independent chunks in your data
#[derive(Debug, Default)]
pub struct ParcodeOptions {
    /// Whether to enable compression for serialized chunks.
    ///
    /// When `true`, the default compression algorithm will be used. The actual algorithm
    /// depends on enabled features (LZ4 if `lz4_flex` is enabled, otherwise no compression).
    use_compression: bool,
}

impl ParcodeOptions {
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
    /// ```rust
    /// use parcode::{Parcode, ParcodeObject};
    /// use serde::{Serialize, Deserialize};
    ///
    /// #[derive(Serialize, Deserialize, ParcodeObject)]
    /// struct MyData { val: i32 }
    /// let my_data = MyData { val: 42 };
    ///
    /// // Enable compression
    /// Parcode::builder()
    ///     .compression(true)
    ///     .write("data.par", &my_data)?;
    /// # std::fs::remove_file("data.par")?;
    /// # Ok::<(), parcode::ParcodeError>(())
    /// ```
    ///
    /// ## Performance Notes
    ///
    /// - LZ4 compression typically achieves 2-3x compression ratios on structured data
    ///   with minimal CPU overhead. For data that doesn't compress well (e.g., already
    ///   compressed images), the overhead may outweigh the benefits.
    pub fn compression(mut self, enable: bool) -> Self {
        self.use_compression = enable;
        self
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
    /// ```rust
    /// use parcode::Parcode;
    ///
    /// let data = vec![1, 2, 3, 4, 5];
    ///
    /// // Write with compression
    /// Parcode::builder()
    ///     .compression(true)
    ///     .write("data_write.par", &data)?;
    /// # std::fs::remove_file("data_write.par")?;
    /// # Ok::<(), parcode::ParcodeError>(())
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
    /// Serializes an object graph to disk with the configured settings.
    ///
    /// This is a convenience wrapper around `write` that handles file creation.
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
    #[cfg(not(target_arch = "wasm32"))]
    pub fn save<T, P>(&self, path: P, root_object: &T) -> Result<()>
    where
        T: ParcodeVisitor + Sync,
        P: AsRef<Path>,
    {
        let file = std::fs::File::create(path)?;
        self.write(file, root_object)
    }

    /// Serializes the object graph to a generic writer (File, `Vec<u8>`, `TcpStream`, etc).
    ///
    /// This method performs the complete serialization pipeline:
    ///
    /// 1. **Graph Construction:** Analyzes the object structure and builds a dependency graph
    /// 2. **Parallel Execution:** Processes independent chunks concurrently using Rayon
    /// 3. **Compression:** Applies compression to each chunk (if enabled)
    /// 4. **I/O:** Writes chunks to the writer in a bottom-up order
    /// 5. **Header Writing:** Appends the global header pointing to the root chunk
    ///
    /// ## Type Parameters
    ///
    /// - `'a`: The lifetime of the object being serialized. The graph borrows from the object,
    ///   enabling zero-copy serialization.
    ///
    /// ## Thread Safety
    ///
    /// - `T`: The type to serialize. Must implement [`ParcodeVisitor`] + `Sync`.
    /// - `W`: The writer type. Must implement `Write` + `Send`.
    ///
    /// ## Parameters
    ///
    /// - `writer`: The destination to write to.
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
    /// - Serialization fails (e.g., bincode error)
    /// - Compression fails
    /// - I/O errors occur during writing to the `writer`
    ///
    /// ## Examples
    ///
    /// ```rust
    /// use parcode::Parcode;
    ///
    /// let data = vec![1, 2, 3];
    /// let mut buffer = Vec::new();
    ///
    /// Parcode::builder()
    ///     .write(&mut buffer, &data)?;
    /// # Ok::<(), parcode::ParcodeError>(())
    /// ```
    pub fn write<'a, T, W>(&self, writer: W, root_object: &'a T) -> Result<()>
    where
        T: ParcodeVisitor + Sync,
        W: Write + Send,
    {
        // 1. Build the Task Graph
        let mut graph = TaskGraph::<'a>::new();
        // The root has no parent (None) and no slot (None).
        root_object.visit(&mut graph, None, None);

        // 2. Prepare the Writer
        let seq_writer = SeqWriter::new(writer);

        // 3. Execute the Graph (Parallel)
        let registry = crate::compression::CompressorRegistry::new();
        let root_child_ref = execute_graph(&graph, &seq_writer, &registry, self.use_compression)?;

        // 4. Write Global Header
        let header = GlobalHeader::new(root_child_ref.offset, root_child_ref.length);
        seq_writer.write_all(&header.to_bytes())?;
        seq_writer.flush()?;

        Ok(())
    }

    /// Serializes an object synchronously (single-threaded).
    ///
    /// This method is useful for:
    /// - Environments where spawning threads is expensive or restricted (WASM, embeddedish).
    /// - Debugging serialization logic without concurrency noise.
    /// - Benchmarking vs Parallel implementation.
    ///
    /// It uses less memory than `write` because it reuses a single compression buffer.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn save_sync<T, P>(&self, path: P, root_object: &T) -> Result<()>
    where
        T: ParcodeVisitor,
        P: AsRef<Path>,
    {
        let file = std::fs::File::create(path)?;
        self.write_sync(file, root_object)
    }

    /// PLACEHOLDER
    pub fn write_sync<'a, T, W>(&self, writer: W, root_object: &'a T) -> Result<()>
    where
        T: ParcodeVisitor,
        W: Write + Send,
    {
        let mut graph = TaskGraph::<'a>::new();
        root_object.visit(&mut graph, None, None);

        let seq_writer = SeqWriter::new(writer);
        let registry = crate::compression::CompressorRegistry::new();

        #[cfg(feature = "parallel")]
        let root_child_ref = crate::executor::execute_graph_sync(
            &graph,
            &seq_writer,
            &registry,
            self.use_compression,
        )?;

        #[cfg(not(feature = "parallel"))]
        let root_child_ref = execute_graph(&graph, &seq_writer, &registry, self.use_compression)?;

        let header = GlobalHeader::new(root_child_ref.offset, root_child_ref.length);
        seq_writer.write_all(&header.to_bytes())?;
        seq_writer.flush()?;

        Ok(())
    }
}
