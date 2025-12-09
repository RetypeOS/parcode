//! Runtime utilities for generated code (Macros).
//! Do not use directly.

use crate::error::Result;
use crate::format::ChildRef;
use crate::graph::{JobConfig, SerializationJob};
use crate::reader::{ChunkNode, ParcodeItem};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::hash::Hash;
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

impl<'a, T: ParcodeItem + Send + Sync + 'a> ParcodeCollectionPromise<'a, T> {
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

#[derive(Debug)]
pub struct ParcodeMapPromise<'a, K, V> {
    node: ChunkNode<'a>,
    _m: PhantomData<(K, V)>,
}

impl<'a, K, V> ParcodeMapPromise<'a, K, V>
where
    K: Hash + Eq + DeserializeOwned,
    V: DeserializeOwned,
{
    /// Constructor interno.
    pub fn new(node: ChunkNode<'a>) -> Self {
        Self {
            node,
            _m: PhantomData,
        }
    }

    /// Loads full map by iterating all shards.
    pub fn load(&self) -> Result<HashMap<K, V>> {
        // 1. Leer número de shards del contenedor
        let container_payload = self.node.read_raw()?;
        let num_shards = if container_payload.len() >= 4 {
            u32::from_le_bytes(container_payload[0..4].try_into().unwrap())
        } else {
            0
        };

        let mut map = HashMap::new();
        if num_shards == 0 {
            return Ok(map);
        }

        // 2. Iterar Shards
        // Usamos children() que devuelve Vec<ChunkNode>
        let shards = self.node.children()?;
        for shard in shards {
            let payload = shard.read_raw()?;
            if payload.len() < 8 {
                continue;
            }

            // Parsear header SOA
            let count = u32::from_le_bytes(payload[0..4].try_into().unwrap()) as usize;

            // Layout: Count(4) + Padding(4) + Hashes(8*N) + Offsets(4*N) + Data
            let offsets_start = 8 + (count * 8);
            let data_start = offsets_start + (count * 4);

            // Leer offsets para iterar datos
            let offsets_bytes = &payload[offsets_start..data_start];

            for i in 0..count {
                let off_bytes = &offsets_bytes[i * 4..(i + 1) * 4];
                let offset = u32::from_le_bytes(off_bytes.try_into().unwrap()) as usize;
                let data_slice = &payload[data_start + offset..];

                // Deserializar par (K, V)
                let (k, v): (K, V) =
                    bincode::serde::decode_from_slice(data_slice, bincode::config::standard())
                        .map_err(|e| crate::ParcodeError::Serialization(e.to_string()))?
                        .0;

                map.insert(k, v);
            }
        }
        Ok(map)
    }

    pub fn get(&self, key: &K) -> Result<Option<V>> {
        // 1. Leer Container Payload (Num Shards)
        let container_payload = self.node.read_raw()?;
        if container_payload.len() < 4 {
            return Ok(None);
        } // Vacío
        let num_shards = u32::from_le_bytes(container_payload[0..4].try_into().unwrap());

        // 2. Hash & Select Shard
        let target_hash = crate::map::hash_key(key);
        let shard_idx = (target_hash as usize) % (num_shards as usize);

        // 3. Load Shard
        let shard = self.node.get_child_by_index(shard_idx)?;
        let payload = shard.read_raw()?;

        if payload.len() < 8 {
            return Ok(None);
        } // Empty shard

        let count = u32::from_le_bytes(payload[0..4].try_into().unwrap()) as usize;
        // Skip 4 bytes padding -> Offset 8

        let hashes_start = 8;
        let hashes_end = hashes_start + (count * 8);
        let offsets_start = hashes_end;
        let data_start = offsets_start + (count * 4);

        // 4. Fast Scan (SIMD Optimized via chunks_exact)
        let hashes_slice = &payload[hashes_start..hashes_end];

        for (i, chunk) in hashes_slice.chunks_exact(8).enumerate() {
            let h = u64::from_le_bytes(chunk.try_into().unwrap());

            if h == target_hash {
                // Candidato. Verificar.
                let offset_bytes = &payload[offsets_start + (i * 4)..];
                let offset = u32::from_le_bytes(offset_bytes[0..4].try_into().unwrap()) as usize;

                let data_slice = &payload[data_start + offset..];

                // Deserializar (K, V)
                // Usamos bincode::deserialize_from slice. Bincode sabe cuándo parar.
                let (stored_key, stored_val): (K, V) =
                    bincode::serde::decode_from_slice(data_slice, bincode::config::standard())
                        .map_err(|e| crate::ParcodeError::Serialization(e.to_string()))?
                        .0;

                if &stored_key == key {
                    return Ok(Some(stored_val));
                }
                // Si no coincide, es una colisión de hash (raro). Seguimos buscando.
            }
        }

        Ok(None)
    }
}

impl<'a, K, V> ParcodeLazyRef<'a> for HashMap<K, V>
where
    K: Hash + Eq + DeserializeOwned + Send + Sync + 'static,
    V: DeserializeOwned + Send + Sync + 'static,
{
    type Lazy = ParcodeMapPromise<'a, K, V>;
    fn create_lazy(node: ChunkNode<'a>) -> Result<Self::Lazy> {
        Ok(ParcodeMapPromise::new(node))
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

impl<'a, T: ParcodeItem + Send + Sync + 'static> ParcodeLazyRef<'a> for Vec<T> {
    type Lazy = ParcodeCollectionPromise<'a, T>;
    fn create_lazy(node: ChunkNode<'a>) -> Result<Self::Lazy> {
        Ok(ParcodeCollectionPromise::new(node))
    }
}
