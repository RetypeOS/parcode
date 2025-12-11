//! Defines the `ParcodeVisitor` trait for graph construction.
//!
//! This module provides the core abstraction that allows types to define how they should
//! be decomposed into nodes in the serialization graph. The visitor pattern is central to
//! Parcode's architecture, enabling flexible and extensible serialization strategies.
//!
//! ## The Visitor Pattern in Parcode
//!
//! The [`ParcodeVisitor`] trait is inspired by the classic Visitor pattern but adapted for
//! graph construction rather than traversal. When you call `object.visit(graph, ...)`, the
//! object:
//!
//! 1. Creates a node for itself in the graph
//! 2. Recursively visits its children (fields marked with `#[parcode(chunkable)]` or `#[parcode(map)]`)
//! 3. Links child nodes to its own node, establishing the dependency graph
//!
//! ## Visitor vs. Serialize
//!
//! While `serde::Serialize` converts objects to bytes, `ParcodeVisitor` converts objects to
//! graph nodes:
//!
//! - **`Serialize`:** `Object → Bytes`
//! - **`ParcodeVisitor`:** `Object → Graph Nodes + Dependencies`
//!
//! The actual serialization to bytes happens later, during graph execution, using the
//! `SerializationJob` trait.
//!
//! ## Two Modes of Visiting
//!
//! The trait defines two visiting modes:
//!
//! ### 1. Standard Visit ([`visit`](ParcodeVisitor::visit))
//!
//! Used when the object is the root or a direct child of another node. The object creates
//! its own node in the graph.
//!
//! ```text
//! Parent Node
//!     ↓
//! [visit creates a new node]
//!     ↓
//! Child Node (this object)
//!     ↓
//! [recursively visits its own children]
//! ```
//!
//! ### 2. Inlined Visit ([`visit_inlined`](ParcodeVisitor::visit_inlined))
//!
//! Used when the object is part of a collection (e.g., an element in `Vec<T>`). The object's
//! local data is already serialized in the parent's payload, so it doesn't create a new node.
//! However, it must still visit its remote children (chunkable fields) and link them to the
//! parent.
//!
//! ```text
//! Vec<MyStruct> Node
//!     ↓
//! [MyStruct instances are inlined in the Vec's payload]
//!     ↓
//! [visit_inlined links remote children to the Vec node]
//!     ↓
//! Remote Child Nodes
//! ```
//!
//! ## Serialization Methods
//!
//! The trait also provides methods for serializing data:
//!
//! - **[`serialize_shallow`](ParcodeVisitor::serialize_shallow):** Serializes only local fields
//!   (ignoring chunkable/map fields). Used when the object is inlined in a collection.
//!
//! - **[`serialize_slice`](ParcodeVisitor::serialize_slice):** Serializes a slice of objects.
//!   The default implementation uses bincode for the entire slice, but types with chunkable
//!   fields should override this to call `serialize_shallow` for each element.
//!
//! ## Derive Macro
//!
//! The `#[derive(ParcodeObject)]` macro automatically implements this trait based on field
//! attributes:
//!
//! - **No attribute:** Field is serialized inline (part of the node's payload)
//! - **`#[parcode(chunkable)]`:** Field becomes a child node
//! - **`#[parcode(map)]`:** Field becomes a sharded map (multiple child nodes)
//!
//! ## Example Implementation
//!
//! ```rust,ignore
//! use parcode::visitor::ParcodeVisitor;
//! use parcode::graph::TaskGraph;
//!
//! struct MyStruct {
//!     local_field: i32,      // Inlined
//!     remote_field: Vec<u8>, // Chunkable (separate node)
//! }
//!
//! impl ParcodeVisitor for MyStruct {
//!     fn visit<'a>(&'a self, graph: &mut TaskGraph<'a>, parent_id: Option<ChunkId>, config: Option<JobConfig>) {
//!         // 1. Create a node for this struct
//!         let my_id = graph.add_node(self.create_job(config));
//!         
//!         // 2. Link to parent if present
//!         if let Some(pid) = parent_id {
//!             graph.link_parent_child(pid, my_id);
//!         }
//!         
//!         // 3. Visit remote children
//!         self.remote_field.visit(graph, Some(my_id), None);
//!     }
//!     
//!     fn create_job<'a>(&'a self, config: Option<JobConfig>) -> Box<dyn SerializationJob<'a> + 'a> {
//!         // Return a job that serializes local_field
//!         Box::new(MyStructJob { data: self })
//!     }
//!     
//!     // ... other methods
//! }
//! ```

use crate::graph::{ChunkId, JobConfig, SerializationJob, TaskGraph};

/// A trait for types that can be structurally visited to build a Parcode [`TaskGraph`].
///
/// This trait is the core abstraction for graph construction in Parcode. It allows types to
/// define how they should be decomposed into nodes and how dependencies between nodes should
/// be established.
///
/// ## Relationship to Other Traits
///
/// - **`serde::Serialize`:** Required for actual byte serialization (used by `SerializationJob`)
/// - **`ParcodeVisitor`:** Defines graph structure (this trait)
/// - **`SerializationJob`:** Executes the actual serialization of a node's payload
/// - **`ParcodeNative`:** Defines deserialization strategy (reader-side)
///
/// ## Automatic Implementation
///
/// This trait is typically implemented automatically via `#[derive(ParcodeObject)]`. Manual
/// implementation is only needed for custom serialization strategies.
///
/// ## Thread Safety
///
/// Implementations must be `Sync` to support parallel graph execution. This is automatically
/// satisfied for most types.
pub trait ParcodeVisitor {
    /// Visits the object and populates the graph with nodes and dependencies.
    ///
    /// This is the primary method for graph construction. It creates a node for the object,
    /// links it to its parent (if any), and recursively visits all remote children.
    ///
    /// ## Lifetimes
    ///
    /// * `'a`: The lifetime of the graph and the data being serialized. The object must outlive
    ///   `'a` to enable zero-copy serialization (the graph holds references to the object's data).
    ///
    /// ## Parameters
    ///
    /// * `graph`: The task graph being constructed. Nodes are added to this graph.
    /// * `parent_id`: The ID of the parent node, if this object is a child of another node.
    ///   `None` indicates this is the root object.
    /// * `config_override`: Optional configuration to override default settings (e.g., compression).
    ///
    /// ## Behavior
    ///
    /// 1. Creates a new node for this object using [`create_job`](Self::create_job)
    /// 2. Links the new node to the parent (if `parent_id` is `Some`)
    /// 3. Recursively calls `visit` on all remote children (chunkable/map fields)
    ///
    /// ## Example
    ///
    /// ```rust,ignore
    /// let mut graph = TaskGraph::new();
    /// my_object.visit(&mut graph, None, None); // Root object, no parent
    /// ```
    fn visit<'a>(
        &'a self,
        graph: &mut TaskGraph<'a>,
        parent_id: Option<ChunkId>,
        config_override: Option<JobConfig>,
    );

    /// Creates the serialization job for this object.
    ///
    /// This method returns a boxed `SerializationJob` that knows how to serialize the object's
    /// local fields (excluding remote children). The job will be executed later during graph
    /// execution.
    ///
    /// ## Parameters
    ///
    /// * `config_override`: Optional configuration to apply to this job (e.g., compression settings).
    ///
    /// ## Returns
    ///
    /// A boxed `SerializationJob` with lifetime `'a`, allowing it to hold references to `self`.
    ///
    /// ## Implementation Note
    ///
    /// The derive macro typically implements this by cloning `self` and wrapping it in a job.
    /// For types with large local fields, consider implementing a custom job that holds references
    /// instead of cloning.
    fn create_job<'a>(
        &'a self,
        config_override: Option<JobConfig>,
    ) -> Box<dyn SerializationJob<'a> + 'a>;

    /// Serializes only the local fields of this object (excluding chunkable/map fields).
    ///
    /// This method is used when the object is part of a collection and its local data needs
    /// to be serialized inline. Remote children are handled separately via `visit_inlined`.
    ///
    /// ## Default Implementation
    ///
    /// The default implementation serializes the entire object using bincode. Types with
    /// chunkable fields should override this to serialize only local fields.
    ///
    /// ## Parameters
    ///
    /// * `writer`: The writer to serialize data into.
    ///
    /// ## Errors
    ///
    /// Returns [`ParcodeError::Serialization`](crate::ParcodeError::Serialization) if
    /// serialization fails.
    ///
    /// ## Example
    ///
    /// ```rust,ignore
    /// // For a struct with local and remote fields:
    /// fn serialize_shallow<W: std::io::Write>(&self, writer: &mut W) -> Result<()> {
    ///     // Only serialize local fields
    ///     bincode::serialize_into(writer, &self.local_field)?;
    ///     Ok(())
    /// }
    /// ```
    fn serialize_shallow<W: std::io::Write>(&self, writer: &mut W) -> crate::error::Result<()>
    where
        Self: serde::Serialize,
    {
        // Default: serialize everything
        crate::internal::bincode::serde::encode_into_std_write(
            self,
            writer,
            crate::internal::bincode::config::standard(),
        )
        .map_err(|e| crate::error::ParcodeError::Serialization(e.to_string()))
        .map(|_| ())
    }

    /// Serializes a slice of objects.
    ///
    /// This method is used to serialize collections of objects. The default implementation
    /// uses bincode to serialize the entire slice, which is efficient for types without
    /// chunkable fields.
    ///
    /// ## Override for Chunkable Types
    ///
    /// Types with chunkable fields should override this method to:
    /// 1. Serialize the slice length
    /// 2. Call `serialize_shallow` for each element
    ///
    /// This ensures that only local fields are serialized inline, while remote children
    /// are handled via `visit_inlined`.
    ///
    /// ## Parameters
    ///
    /// * `slice`: The slice of objects to serialize.
    /// * `writer`: The writer to serialize data into.
    ///
    /// ## Errors
    ///
    /// Returns [`ParcodeError::Serialization`](crate::ParcodeError::Serialization) if
    /// serialization fails.
    fn serialize_slice<W: std::io::Write>(
        slice: &[Self],
        writer: &mut W,
    ) -> crate::error::Result<()>
    where
        Self: Sized + serde::Serialize,
    {
        crate::internal::bincode::serde::encode_into_std_write(
            slice,
            writer,
            crate::internal::bincode::config::standard(),
        )
        .map_err(|e| crate::error::ParcodeError::Serialization(e.to_string()))
        .map(|_| ())
    }

    /// Visits the object as an inlined item within a container (e.g., `Vec`).
    ///
    /// In this mode, the object does NOT create a new node for itself, as its local data
    /// is already serialized in the container's payload (via `serialize_shallow`). However,
    /// it must still visit its remote children (chunkable fields) and link them to the
    /// container's node.
    ///
    /// ## When is this called?
    ///
    /// This method is called when:
    /// - The object is an element in a `Vec<T>` where `T` has chunkable fields
    /// - The object is a value in a `HashMap<K, V>` where `V` has chunkable fields
    ///
    /// ## Default Implementation
    ///
    /// The default implementation does nothing, which is correct for types without chunkable
    /// fields. Types with chunkable fields must override this to visit their remote children.
    ///
    /// ## Parameters
    ///
    /// * `graph`: The task graph being constructed.
    /// * `parent_id`: The ID of the container node (e.g., the `Vec` node).
    /// * `config_override`: Optional configuration override.
    ///
    /// ## Example
    ///
    /// ```rust,ignore
    /// // For a struct with a chunkable field:
    /// fn visit_inlined<'a>(&'a self, graph: &mut TaskGraph<'a>, parent_id: ChunkId, config: Option<JobConfig>) {
    ///     // Visit remote children and link them to parent_id
    ///     self.remote_field.visit(graph, Some(parent_id), None);
    /// }
    /// ```
    ///
    /// Note: Users deriving `ParcodeObject` do not need to implement this manually.
    fn visit_inlined<'a>(
        &'a self,
        _graph: &mut TaskGraph<'a>,
        _parent_id: ChunkId,
        _config_override: Option<JobConfig>,
    ) {
        // Default: Do nothing (no children to visit or link).
        // Override this for structs with #[parcode(chunkable)] fields.
    }
}
