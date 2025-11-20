#[cfg(feature = "lz4_flex")]
use crate::compression::Lz4Compressor;
use crate::compression::{Compressor, NoCompression};
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
        root_object.visit(&mut graph, None);

        let writer = SeqWriter::create(path)?;

        let compressor: Box<dyn Compressor> = if self.use_compression {
            #[cfg(feature = "lz4_flex")]
            {
                Box::new(Lz4Compressor)
            }
            #[cfg(not(feature = "lz4_flex"))]
            {
                Box::new(NoCompression)
            }
        } else {
            Box::new(NoCompression)
        };

        let root_child_ref = execute_graph(&graph, &writer, compressor.as_ref())?;

        let header = GlobalHeader::new(root_child_ref.offset, root_child_ref.length);
        writer.write_all(&header.to_bytes())?;
        writer.flush()?;

        Ok(())
    }
}
