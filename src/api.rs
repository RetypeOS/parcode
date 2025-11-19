use std::path::Path;
use crate::error::Result;
use crate::graph::TaskGraph;
use crate::executor::execute_graph;
use crate::io::SeqWriter;
use crate::compression::NoCompression;
use crate::visitor::ParcodeVisitor;
use crate::format::GlobalHeader;

/// The main entry point for saving data.
#[derive(Debug)]
pub struct Parcode;

impl Parcode {
    /// Saves a complex object tree to a file using the parallel graph engine.
    ///
    /// # Arguments
    /// * `path`: Destination file path.
    /// * `root_object`: The root of the data structure. Must derive `ParcodeObject`.
    pub fn save<T, P>(path: P, root_object: &T) -> Result<()>
    where
        T: ParcodeVisitor + Sync, // Sync needed because we share ref in graph building (if optimized)
        P: AsRef<Path>,
    {
        let path = path.as_ref();
        
        // 1. Build the Task Graph (Discovery Phase)
        // This runs on the calling thread. It's fast memory walking.
        let mut graph = TaskGraph::new();
        root_object.visit(&mut graph, None); // Root has no parent

        // 2. Setup I/O
        let writer = SeqWriter::create(path)?;
        
        // 3. Configure Compression (Default for now)
        let compressor = NoCompression; // In future, pass config in `save_with_config`

        // 4. Execute Graph (Parallel Phase)
        // This handles all serialization and writing.
        let root_child_ref = execute_graph(&graph, &writer, &compressor)?;

        // 5. Finalize: Write Global Header
        // We write this at the very end of the file.
        let header = GlobalHeader::new(root_child_ref.offset, root_child_ref.length);
        writer.write_all(&header.to_bytes())?;
        writer.flush()?;

        Ok(())
    }
}