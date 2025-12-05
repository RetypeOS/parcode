// src/rt.rs

//! Runtime utilities for generated code (Macros).
//! Do not use directly.

use crate::error::Result;
use crate::format::ChildRef;
use crate::graph::{JobConfig, SerializationJob};
use crate::reader::ChunkNode;
use serde::de::DeserializeOwned;
use std::marker::PhantomData;

// --- EXISTING CONFIG WRAPPER ---

/// Wrapper that injects configuration into an existing Job.
#[derive(Debug)]
pub struct ConfiguredJob<'a, J: ?Sized> {
    config: JobConfig,
    inner: Box<J>,
    _marker: PhantomData<&'a ()>,
}

impl<'a, J: SerializationJob<'a> + ?Sized> ConfiguredJob<'a, J> {
    pub fn new(inner: Box<J>, config: JobConfig) -> Self {
        Self {
            inner,
            config,
            _marker: PhantomData,
        }
    }
}

impl<'a, J: SerializationJob<'a> + ?Sized> SerializationJob<'a> for ConfiguredJob<'a, J> {
    fn execute(&self, children_refs: &[ChildRef]) -> Result<Vec<u8>> {
        self.inner.execute(children_refs)
    }

    fn estimated_size(&self) -> usize {
        self.inner.estimated_size()
    }

    fn config(&self) -> JobConfig {
        self.config
    }
}

// --- NEW LAZY MIRROR INFRASTRUCTURE ---

/// Trait implemented by types that support Lazy Mirroring.
///
/// This trait acts as a bridge between the original type `T` and its generated
/// lazy counterpart `T::Lazy`.
pub trait ParcodeLazyRef<'a>: Sized {
    /// The Mirror Type.
    /// For primitives, it is `ParcodePromise<'a, T>`.
    /// For structs deriving `ParcodeObject`, it is `StructNameLazy<'a>`.
    type Lazy;

    /// Creates the lazy view from a graph node.
    fn create_lazy(node: ChunkNode<'a>) -> Result<Self::Lazy>;
}

/// A terminal promise for a single value.
///
/// Use `.load()` to trigger deserialization.
#[derive(Debug, Clone)]
pub struct ParcodePromise<'a, T> {
    node: ChunkNode<'a>,
    _m: PhantomData<T>,
}

impl<'a, T: DeserializeOwned> ParcodePromise<'a, T> {
    /// Internal constructor.
    pub fn new(node: ChunkNode<'a>) -> Self {
        Self {
            node,
            _m: PhantomData,
        }
    }

    /// Loads the data from disk/memory.
    pub fn load(&self) -> Result<T> {
        self.node.decode()
    }
}

/// A promise for a collection (Vector).
///
/// Supports partial loading and random access via `.get(index)`.
/// Specialized lazy field for collections (Vec) allowing partial access.
#[derive(Debug, Clone)]
pub struct ParcodeCollectionPromise<'a, T> {
    node: ChunkNode<'a>,
    _m: PhantomData<T>,
}

impl<'a, T: DeserializeOwned + Send + Sync + 'a> ParcodeCollectionPromise<'a, T> {
    /// Internal constructor.
    pub fn new(node: ChunkNode<'a>) -> Self {
        Self {
            node,
            _m: PhantomData,
        }
    }

    /// Loads the entire collection into memory.
    pub fn load(&self) -> Result<Vec<T>> {
        self.node.decode_parallel_collection()
    }

    /// Retrieves a single item without loading the whole collection.
    /// Uses O(1) arithmetic navigation.
    pub fn get(&self, index: usize) -> Result<T> {
        self.node.get(index)
    }

    /// Returns a streaming iterator.
    pub fn iter(&self) -> Result<impl Iterator<Item = Result<T>> + 'a> {
        self.node.clone().iter()
    }
}

// --- BLANKET IMPLEMENTATIONS FOR PRIMITIVES ---

macro_rules! impl_lazy_primitive {
    ($($t:ty),*) => {
        $(
            impl<'a> ParcodeLazyRef<'a> for $t {
                type Lazy = ParcodePromise<'a, $t>;
                fn create_lazy(node: ChunkNode<'a>) -> Result<Self::Lazy> {
                    Ok(ParcodePromise::new(node))
                }
            }
        )*
    }
}

impl_lazy_primitive!(u8, u16, u32, u64, i8, i16, i32, i64, f32, f64, bool, String);

// --- BLANKET IMPLEMENTATION FOR VECTORS ---

impl<'a, T: DeserializeOwned + Send + Sync + 'static> ParcodeLazyRef<'a> for Vec<T> {
    type Lazy = ParcodeCollectionPromise<'a, T>;
    fn create_lazy(node: ChunkNode<'a>) -> Result<Self::Lazy> {
        Ok(ParcodeCollectionPromise::new(node))
    }
}
