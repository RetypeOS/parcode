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
    /// The unique identifier for this node.
    pub id: ChunkId,
    /// The ID of the parent node (if any).
    /// Currently, the graph is a tree, so a node has at most one parent.
    pub parent: Option<ChunkId>,
    /// The number of dependencies (children) that must complete before this node can run.
    pub atomic_deps: AtomicUsize,

    /// The job to execute. Note the `+ 'a` bound.
    pub job: Box<dyn SerializationJob<'a> + 'a>,

    /// The slot index in the parent's `completed_children` vector.
    /// This allows the child to write its result directly to the correct position.
    pub parent_slot: Option<usize>,

    /// A list of results from completed children.
    /// This is populated as children finish execution.
    /// We use `Option` to allow random-access insertion without resizing issues,
    /// though we push in order during linking.
    pub completed_children: Mutex<Vec<Option<ChildRef>>>,
}

impl<'a> Node<'a> {
    /// Creates a new Node.
    pub fn new(id: ChunkId, job: Box<dyn SerializationJob<'a> + 'a>) -> Self {
        Self {
            id,
            parent: None,
            parent_slot: None,
            atomic_deps: AtomicUsize::new(0),
            job,
            completed_children: Mutex::new(Vec::new()),
        }
    }

    /// Registers a completed child's result.
    ///
    /// This is called by the executor when a child node finishes.
    pub fn register_child_result(&self, slot: usize, child_ref: ChildRef) -> Result<()> {
        let mut lock = self
            .completed_children
            .lock()
            .map_err(|_| ParcodeError::Internal(format!("Mutex poisoned on node {:?}", self.id)))?;

        let entry = lock.get_mut(slot).ok_or_else(|| {
            ParcodeError::Internal(format!(
                "Slot {} out of bounds for node {:?}",
                slot, self.id
            ))
        })?;
        *entry = Some(child_ref);
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
    /// Creates a new, empty `TaskGraph`.
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    /// Adds a new node to the graph.
    ///
    /// Returns the `ChunkId` of the newly created node.
    pub fn add_node(&mut self, job: Box<dyn SerializationJob<'a> + 'a>) -> ChunkId {
        let id = ChunkId::new(u32::try_from(self.nodes.len()).unwrap_or(u32::MAX));
        let node = Node::new(id, job);
        self.nodes.push(node);
        id
    }

    /// Links a parent node to a child node.
    ///
    /// This increments the parent's dependency count and sets the child's parent pointer.
    pub fn link_parent_child(&mut self, parent_id: ChunkId, child_id: ChunkId) {
        let parent_node_idx = parent_id.as_u32() as usize;
        let child_node_idx = child_id.as_u32() as usize;
        let parent_node = self
            .nodes
            .get(parent_node_idx)
            .expect("Parent node not found");
        parent_node.atomic_deps.fetch_add(1, Ordering::SeqCst);

        // Reserve slot in parent
        let slot = {
            let mut guard = parent_node
                .completed_children
                .lock()
                .expect("Mutex poisoned");
            let idx = guard.len();
            guard.push(None);
            idx
        };

        let child_node = self
            .nodes
            .get_mut(child_node_idx)
            .expect("Child node not found");
        child_node.parent = Some(parent_id);
        child_node.parent_slot = Some(slot);
    }

    /// Retrieves a reference to a node by its ID.
    ///
    /// # Panics
    ///
    /// Panics if the `id` does not exist in the graph.
    pub fn get_node(&self, id: ChunkId) -> &Node<'a> {
        self.nodes
            .get(id.as_u32() as usize)
            .expect("TaskGraph invariant violated: Node ID out of bounds")
    }

    /// Returns true if the graph has no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Returns the number of nodes in the graph.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Returns a slice containing all nodes in the graph.
    pub fn nodes(&self) -> &[Node<'a>] {
        &self.nodes
    }

    /// Replaces the job of a node.
    ///
    /// This is used for optimization patterns where a node ID is needed to process
    /// dependencies (children) before the final job (consuming the data) can be fully constructed.
    ///
    /// # Panics
    ///
    /// Panics if the `id` does not exist in the graph. This is considered an internal
    /// invariant violation.
    pub fn replace_job(&mut self, id: ChunkId, new_job: Box<dyn SerializationJob<'a> + 'a>) {
        let node_idx = id.as_u32() as usize;
        let node = self
            .nodes
            .get_mut(node_idx)
            .expect("TaskGraph invariant violated: Node ID not found during job replacement");
        node.job = new_job;
    }
}

// Manual Default impl to avoid restrictive bounds on 'a
impl<'a> Default for TaskGraph<'a> {
    fn default() -> Self {
        Self::new()
    }
}
