//! High-level API for Parcode.
//!
//! This module provides the primary entry points for reading and writing Parcode files.
//! It offers a builder-style interface for configuration and simple functions for
//! common operations.

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
/// # Example
///
/// ```rust,ignore
/// use parcode::Parcode;
///
/// let data = vec![1, 2, 3];
/// Parcode::save("data.par", &data).unwrap();
/// let loaded: Vec<i32> = Parcode::read("data.par").unwrap();
/// ```
#[derive(Debug, Default)]
pub struct Parcode {
    use_compression: bool,
}

impl Parcode {
    /// Creates a new `Parcode` builder with default settings.
    pub fn builder() -> Self {
        Self::default()
    }

    /// Enables or disables compression.
    ///
    /// If enabled, the default compression algorithm (usually LZ4 if enabled) will be used.
    pub fn compression(mut self, enable: bool) -> Self {
        self.use_compression = enable;
        self
    }

    /// Reads an object from a file, automatically selecting the optimal
    /// reconstruction strategy (Parallel for collections, Sequential for leaves).
    pub fn read<T, P>(path: P) -> Result<T>
    where
        T: ParcodeNative, // Constraint ensures we use the specialized logic
        P: AsRef<Path>,
    {
        let reader = ParcodeReader::open(path)?;
        let root = reader.root()?;

        // Dispatches to Vec::from_node (Parallel) or T::from_node (Simple)
        T::from_node(&root)
    }

    /// Saves an object to a file using default settings.
    ///
    /// This is a convenience wrapper around `write`.
    pub fn save<T, P>(path: P, root_object: &T) -> Result<()>
    where
        T: ParcodeVisitor + Sync,
        P: AsRef<Path>,
    {
        Self::default().write(path, root_object)
    }

    /// Serializes the object graph to disk using Zero-Copy where possible.
    /// The `root_object` must outlive the function call (implied).
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
