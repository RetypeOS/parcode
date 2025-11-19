use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use super::id::ChunkId;
use super::job::SerializationJob;
use crate::format::ChildRef;

/// A single node in the dependency graph.
///
/// # Lifecycle
/// 1. **Created:** Dependencies are counted during the Visitor pass.
/// 2. **Waiting:** `atomic_deps > 0`. Children are being processed.
/// 3. **Ready:** `atomic_deps == 0`. Pushed to the Worker Queue.
/// 4. **Done:** Executed, and result (`ChildRef`) pushed to parent's `completed_children`.
#[derive(Debug)]
pub struct Node {
    /// The unique ID of this node.
    pub id: ChunkId,

    /// The ID of the parent node. 
    /// `None` if this is the Root node.
    pub parent: Option<ChunkId>,

    /// The number of children chunks that must be written before this node can run.
    /// This is decremented atomically as children finish.
    pub atomic_deps: AtomicUsize,

    /// The actual data/logic to execute.
    /// Wrapped in Box for type erasure.
    pub job: Box<dyn SerializationJob>,

    /// Storage for the results of the children.
    /// The parent needs these `ChildRef`s to build its Footer table.
    /// Protected by Mutex because children might finish concurrently.
    pub completed_children: Mutex<Vec<(ChunkId, ChildRef)>>,
}

impl Node {
    /// PLACEHOLDER
    pub fn new(id: ChunkId, job: Box<dyn SerializationJob>) -> Self {
        Self {
            id,
            parent: None,
            atomic_deps: AtomicUsize::new(0),
            job,
            completed_children: Mutex::new(Vec::new()),
        }
    }
    
    /// Adds a completed child reference to this node's collection.
    pub fn register_child_result(&self, child_id: ChunkId, child_ref: ChildRef) {
        let mut lock = self.completed_children.lock().expect("Mutex poisoned");
        lock.push((child_id, child_ref));
    }
}

/// The container for the entire dependency graph.
/// Acts as an Arena allocator for Nodes.
#[derive(Debug)]
pub struct TaskGraph {
    /// All nodes, indexed by their `ChunkId.as_u32()`.
    nodes: Vec<Node>,
}

impl TaskGraph {
    /// Creates a new empty graph.
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    /// Adds a new node to the graph and returns its assigned ID.
    pub fn add_node(&mut self, job: Box<dyn SerializationJob>) -> ChunkId {
        let id = ChunkId::new(self.nodes.len() as u32);
        let node = Node::new(id, job);
        self.nodes.push(node);
        id
    }

    /// Establishes a Parent -> Child relationship.
    /// This increments the parent's dependency counter.
    pub fn link_parent_child(&mut self, parent_id: ChunkId, child_id: ChunkId) {
        // 1. Increment Parent's dependency counter.
        let parent_node = &self.nodes[parent_id.as_u32() as usize];
        parent_node.atomic_deps.fetch_add(1, Ordering::Relaxed);

        // 2. Set Child's parent pointer.
        // Note: We need mutable access to setting the parent, but we established 
        // `nodes` as a Vec. Since we are in the "Build Phase" (single threaded visitor),
        // we can mutate freely.
        let child_node = &mut self.nodes[child_id.as_u32() as usize];
        child_node.parent = Some(parent_id);
    }

    /// Returns a reference to a node.
    pub fn get_node(&self, id: ChunkId) -> &Node {
        &self.nodes[id.as_u32() as usize]
    }

    /// Returns the total number of nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    
    /// Returns the list of all nodes. 
    /// Used by the executor to find initial leaves (deps == 0).
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }
}