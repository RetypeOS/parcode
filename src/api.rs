//! PLACEHOLDER

use crate::error::Result;
use crate::executor::execute_graph;
use crate::format::GlobalHeader;
use crate::graph::TaskGraph;
use crate::io::SeqWriter;
use crate::reader::{ParcodeNative, ParcodeReader};
use crate::visitor::ParcodeVisitor;
use std::path::Path;

/// PLACEHOLDER
#[derive(Debug, Default)]
pub struct Parcode {
    use_compression: bool,
}

impl Parcode {
    /// PLACEHOLDER
    pub fn builder() -> Self {
        Self::default()
    }

    /// PLACEHOLDER
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
    /// PLACEHOLDER
    pub fn save<T, P>(path: P, root_object: &T) -> Result<()>
    where
        T: ParcodeVisitor + Sync,
        P: AsRef<Path>,
    {
        Self::default().write(path, root_object)
    }
    /// PLACEHOLDER
    pub fn write<T, P>(&self, path: P, root_object: &T) -> Result<()>
    where
        T: ParcodeVisitor + Sync,
        P: AsRef<Path>,
    {
        let path = path.as_ref();
        let mut graph = TaskGraph::new();
        
        // Visit inicial con Config Default (None)
        root_object.visit(&mut graph, None, None);

        let writer = SeqWriter::create(path)?;
        
        // Instanciamos el Registro con los defaults del sistema
        let registry = crate::compression::CompressorRegistry::new();

        // Ejecutamos
        let root_child_ref = execute_graph(&graph, &writer, &registry)?;

        let header = GlobalHeader::new(root_child_ref.offset, root_child_ref.length);
        writer.write_all(&header.to_bytes())?;
        writer.flush()?;

        Ok(())
    }
}
