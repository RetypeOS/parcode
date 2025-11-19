use crate::visitor::ParcodeVisitor;
use crate::graph::{TaskGraph, ChunkId, SerializationJob};
use crate::error::{Result, ParcodeError};
use crate::format::ChildRef;
use serde::Serialize;

// Configuración heurística
const TARGET_SHARD_SIZE_BYTES: usize = 128 * 1024; // 128 KB por chunk físico

/// A Job that serializes a specific slice of a Vector.
/// This restores the linear write performance by batching items.
#[derive(Clone)]
struct VecShardJob<T> {
    data: Vec<T>, // In prod: use Arc<[T]> to avoid clone, keeping Vec<T> for simplicity in prototype
}

impl<T> SerializationJob for VecShardJob<T>
where T: Serialize + Send + Sync + 'static 
{
    fn execute(&self, _children_refs: &[ChildRef]) -> Result<Vec<u8>> {
        // We serialize the WHOLE slice as one payload.
        // Note: If items inside generated children nodes, 'children_refs' contains them.
        // Standard bincode behavior for Vec is [len, item1, item2...]
        // Here we just dump the items one after another or as a sequence?
        // To be safe and compatible with "Vec" deserialization, we should serialize as a sequence.
        // BUT, since this is a shard, the Reader needs to know how to stitch them.
        // For V2 simplicity: We serialize the slice standardly.
        
        bincode::serde::encode_to_vec(&self.data, bincode::config::standard())
            .map_err(|e| ParcodeError::Serialization(e.to_string()))
    }

    fn estimated_size(&self) -> usize {
        self.data.len() * std::mem::size_of::<T>()
    }
    
    fn as_any(&self) -> &dyn std::any::Any { self }
}

impl<T> ParcodeVisitor for Vec<T>
where 
    T: ParcodeVisitor + Clone + Send + Sync + 'static + Serialize
{
    fn visit(&self, graph: &mut TaskGraph, parent_id: Option<ChunkId>) {
        // 1. Create the Container Node (The "Virtual" Vec)
        // Its payload is empty, it just points to the Shards.
        let container_job = Box::new(VecContainerJob);
        let my_id = graph.add_node(container_job);
        
        if let Some(pid) = parent_id {
            graph.link_parent_child(pid, my_id);
        }

        if self.is_empty() { return; }

        // 2. Calculate Sharding Strategy
        // We assume T size. If T is a complex struct, this is just a lower bound guess.
        let item_size_guess = std::mem::size_of::<T>().max(1); 
        let items_per_shard = (TARGET_SHARD_SIZE_BYTES / item_size_guess).max(1).min(self.len());

        // 3. Create Shards
        for chunk_slice in self.chunks(items_per_shard) {
            // Create a Job for this shard
            // Note: We are cloning the slice. In high-opt prod, we'd use Arc/Slice references.
            let shard_data = chunk_slice.to_vec(); 
            let shard_job = Box::new(VecShardJob { data: shard_data });
            
            let shard_id = graph.add_node(shard_job);
            graph.link_parent_child(my_id, shard_id);

            // 4. Crucial: Visit items inside the shard!
            // Even though they are serialized in the shard payload, 
            // they might have their OWN chunkable fields (children).
            // Those children must become dependencies of the SHARD, not the Vec container.
            for item in chunk_slice {
                item.visit(graph, Some(shard_id));
            }
        }
    }

    fn create_job(&self) -> Box<dyn SerializationJob> {
        Box::new(VecContainerJob)
    }
}

#[derive(Clone)]
struct VecContainerJob;

impl SerializationJob for VecContainerJob {
    fn execute(&self, _children: &[ChildRef]) -> Result<Vec<u8>> {
        // The container itself writes nothing, just holds the table of shards.
        Ok(Vec::new())
    }
    fn estimated_size(&self) -> usize { 0 }
    fn as_any(&self) -> &dyn std::any::Any { self }
}