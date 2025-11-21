use super::id::ChunkId;
use super::job::SerializationJob;
use crate::format::ChildRef;
use crate::{ParcodeError, Result};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A single node in the dependency graph.
///
/// It holds a `SerializationJob` which may contain references to user data (`'a`).
#[derive(Debug)]
pub struct Node<'a> {
    /// PLACEHOLDER
    pub id: ChunkId,
    /// PLACEHOLDER
    pub parent: Option<ChunkId>,
    /// PLACEHOLDER
    pub atomic_deps: AtomicUsize,

    /// The job to execute. Note the `+ 'a` bound.
    pub job: Box<dyn SerializationJob<'a> + 'a>,

    /// PLACEHOLDER
    pub completed_children: Mutex<Vec<(ChunkId, ChildRef)>>,
}

impl<'a> Node<'a> {
    /// PLACEHOLDER
    pub fn new(id: ChunkId, job: Box<dyn SerializationJob<'a> + 'a>) -> Self {
        Self {
            id,
            parent: None,
            atomic_deps: AtomicUsize::new(0),
            job,
            completed_children: Mutex::new(Vec::new()),
        }
    }

    /// PLACEHOLDER
    pub fn register_child_result(&self, child_id: ChunkId, child_ref: ChildRef) -> Result<()> {
        let mut lock = self
            .completed_children
            .lock()
            .map_err(|_| ParcodeError::Internal(format!("Mutex poisoned on node {:?}", self.id)))?;
        lock.push((child_id, child_ref));
        Ok(())
    }
}

/// The container for the entire dependency graph.
///
/// Acts as an Arena allocator for Nodes. The graph lifetime `'a` is tied
/// to the input data provided by the user in `Parcode::save`.
#[derive(Debug)]
pub struct TaskGraph<'a> {
    nodes: Vec<Node<'a>>,
}

impl<'a> TaskGraph<'a> {
    /// PLACEHOLDER
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    /// PLACEHOLDER
    pub fn add_node(&mut self, job: Box<dyn SerializationJob<'a> + 'a>) -> ChunkId {
        let id = ChunkId::new(self.nodes.len() as u32);
        let node = Node::new(id, job);
        self.nodes.push(node);
        id
    }

    /// PLACEHOLDER
    pub fn link_parent_child(&mut self, parent_id: ChunkId, child_id: ChunkId) {
        let parent_node = &self.nodes[parent_id.as_u32() as usize];
        parent_node.atomic_deps.fetch_add(1, Ordering::SeqCst);

        let child_node = &mut self.nodes[child_id.as_u32() as usize];
        child_node.parent = Some(parent_id);
    }

    /// PLACEHOLDER
    pub fn get_node(&self, id: ChunkId) -> &Node<'a> {
        &self.nodes[id.as_u32() as usize]
    }

    /// PLACEHOLDER
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// PLACEHOLDER
    pub fn nodes(&self) -> &[Node<'a>] {
        &self.nodes
    }
}

// Manual Default impl to avoid restrictive bounds on 'a
impl<'a> Default for TaskGraph<'a> {
    fn default() -> Self {
        Self::new()
    }
}
