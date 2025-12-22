//! Runtime utilities for generated code (Macros).
//! Do not use directly.

use crate::error::Result;
use crate::format::ChildRef;
use crate::graph::{JobConfig, SerializationJob};
use crate::reader::{ChunkNode, ParcodeItem};
use serde::de::DeserializeOwned;
use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::Hash;
use std::io::Cursor;
use std::marker::PhantomData;
use std::vec::IntoIter;

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

    /// Creates the lazy view from a data stream (Inside Vec access).
    /// This allows scanning headers without full deserialization.
    fn read_lazy_from_stream(
        reader: &mut Cursor<&[u8]>,
        children: &mut IntoIter<ChunkNode<'a>>,
    ) -> Result<Self::Lazy>;
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

    /// Returns the number of items in the collection.
    ///
    /// This is an O(1) operation that reads the length from the chunk header.
    pub fn len(&self) -> usize {
        usize::try_from(self.node.len()).unwrap_or(usize::MAX)
    }

    /// Returns true if the collection has no items.
    pub fn is_empty(&self) -> bool {
        self.node.is_empty()
    }

    /// Returns a streaming iterator.
    pub fn iter(&self) -> Result<impl Iterator<Item = Result<T>> + 'a> {
        self.node.clone().iter() // TODO: This must be checked to optimize.
    }
}

impl<'a, T> ParcodeCollectionPromise<'a, T>
where
    T: ParcodeItem + Send + Sync + ParcodeLazyRef<'a> + 'a,
{
    /// Retrieves a Lazy Proxy for the item at `index`.
    ///
    /// This allows accessing local fields of the item without triggering the loading
    /// of its heavy dependencies.
    pub fn get_lazy(&self, index: usize) -> Result<T::Lazy> {
        // 1. Resolve shard using internal helper exposed via crate
        let (shard_node, index_in_shard) = self.node.locate_shard_item(index)?;

        // 2. Prepare readers
        let payload = shard_node.read_raw()?;
        let mut cursor = Cursor::new(payload.as_ref());
        let children_vec = shard_node.children()?;
        let mut children_iter = children_vec.into_iter();

        // Skip Vector Length (u64)
        // Shards created by ParcodeVisitor::serialize_slice always have a length prefix.
        let _len: u64 =
            bincode::serde::decode_from_std_read(&mut cursor, bincode::config::standard())
                .map_err(|e| crate::ParcodeError::Serialization(e.to_string()))?;

        // 3. Scan and Skip previous items
        // We read them as Lazy objects and discard them.
        for _ in 0..index_in_shard {
            let _ = T::read_lazy_from_stream(&mut cursor, &mut children_iter)?;
        }

        // 4. Read the target item as Lazy
        T::read_lazy_from_stream(&mut cursor, &mut children_iter)
    }

    /// Returns the first item as a lazy mirror, or None if empty.
    pub fn first(&self) -> Result<Option<T::Lazy>> {
        if self.is_empty() {
            Ok(None)
        } else {
            Ok(Some(self.get_lazy(0)?))
        }
    }

    /// Returns the last item as a lazy mirror, or None if empty.
    pub fn last(&self) -> Result<Option<T::Lazy>> {
        let len = self.len();
        if len == 0 {
            Ok(None)
        } else {
            Ok(Some(self.get_lazy(len - 1)?))
        }
    }

    /// Returns a lazy streaming iterator over the collection.
    ///
    /// This is highly efficient: it loads shards sequentially and processes items
    /// without re-reading bytes. Ideal for scanning large datasets.
    pub fn iter_lazy(&self) -> Result<ParcodeLazyIterator<'a, T>> {
        ParcodeLazyIterator::new(self.node.clone())
    }
}

/// A promise for a `HashMap` that supports lazy loading and efficient lookups.
///
/// This type wraps a `ChunkNode` representing a `HashMap` and provides methods to:
/// - Load the entire map into memory
/// - Perform O(1) lookups by key without loading the entire map
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
    /// Internal constructor.
    pub fn new(node: ChunkNode<'a>) -> Self {
        Self {
            node,
            _m: PhantomData,
        }
    }

    /// Loads the full map by iterating all shards.
    ///
    /// This reconstructs the entire `HashMap` in memory by:
    /// 1. Reading the number of shards from the container
    /// 2. Iterating over each shard and deserializing its entries
    /// 3. Merging all entries into a single `HashMap`
    pub fn load(&self) -> Result<HashMap<K, V>> {
        // 1. Read number of shards from container
        let container_payload = self.node.read_raw()?;
        let num_shards = if container_payload.len() >= 4 {
            u32::from_le_bytes(
                container_payload
                    .get(0..4)
                    .ok_or_else(|| crate::ParcodeError::Format("Payload too short".into()))?
                    .try_into()
                    .map_err(|_| crate::ParcodeError::Format("Failed to read num_shards".into()))?,
            )
        } else {
            0
        };

        let mut map = HashMap::new();
        if num_shards == 0 {
            return Ok(map);
        }

        // 2. Iterate over shards
        // Use children() which returns Vec<ChunkNode>
        let shards = self.node.children()?;
        for shard in shards {
            let payload = shard.read_raw()?;
            if payload.len() < 8 {
                continue;
            }

            // Parse SOA header
            let count = u32::from_le_bytes(
                payload
                    .get(0..4)
                    .ok_or_else(|| {
                        crate::ParcodeError::Format("Payload too short for count".into())
                    })?
                    .try_into()
                    .map_err(|_| crate::ParcodeError::Format("Failed to read count".into()))?,
            ) as usize;

            // Layout: Count(4) + Padding(4) + Hashes(8*N) + Offsets(4*N) + Data
            let offsets_start = 8 + (count * 8);
            let data_start = offsets_start + (count * 4);

            // Read offsets to iterate data
            let offsets_bytes = payload
                .get(offsets_start..data_start)
                .ok_or_else(|| crate::ParcodeError::Format("Offsets out of bounds".into()))?;

            for i in 0..count {
                let off_bytes = offsets_bytes.get(i * 4..(i + 1) * 4).ok_or_else(|| {
                    crate::ParcodeError::Format("Offset index out of bounds".into())
                })?;
                let offset = u32::from_le_bytes(
                    off_bytes
                        .try_into()
                        .map_err(|_| crate::ParcodeError::Format("Failed to read offset".into()))?,
                ) as usize;
                let data_slice = payload.get(data_start + offset..).ok_or_else(|| {
                    crate::ParcodeError::Format("Data slice out of bounds".into())
                })?;

                // Deserialize (K, V) pair
                let (k, v): (K, V) =
                    bincode::serde::decode_from_slice(data_slice, bincode::config::standard())
                        .map_err(|e| crate::ParcodeError::Serialization(e.to_string()))?
                        .0;

                map.insert(k, v);
            }
        }
        Ok(map)
    }

    /// Performs a fast lookup for a single key without loading the entire map.
    ///
    /// This method:
    /// 1. Computes the hash of the key
    /// 2. Determines which shard contains the key (via modulo)
    /// 3. Loads only that shard
    /// 4. Scans the shard's hash array for matches
    /// 5. Verifies the key on hash collision and returns the value
    ///
    /// # Performance
    /// - O(1) shard selection
    /// - O(N/S) linear scan within shard (where S = number of shards)
    /// - SIMD-optimized hash comparison on supported platforms
    pub fn get(&self, key: &K) -> Result<Option<V>> {
        // 1. Read Container Payload (Num Shards)
        let container_payload = self.node.read_raw()?;
        if container_payload.len() < 4 {
            return Ok(None);
        } // Empty
        let num_shards = u32::from_le_bytes(
            container_payload
                .get(0..4)
                .ok_or_else(|| crate::ParcodeError::Format("Payload too short".into()))?
                .try_into()
                .map_err(|_| crate::ParcodeError::Format("Failed to read num_shards".into()))?,
        );

        // 2. Hash & Select Shard
        let target_hash = crate::map::hash_key(key);
        let shard_idx = usize::try_from(target_hash).unwrap_or(0) % (num_shards as usize);

        // 3. Load Shard
        let shard = self.node.get_child_by_index(shard_idx)?;
        let payload = shard.read_raw()?;

        if payload.len() < 8 {
            return Ok(None);
        } // Empty shard

        let count = u32::from_le_bytes(
            payload
                .get(0..4)
                .ok_or_else(|| crate::ParcodeError::Format("Payload too short for count".into()))?
                .try_into()
                .map_err(|_| crate::ParcodeError::Format("Failed to read count".into()))?,
        ) as usize;
        // Skip 4 bytes padding -> Offset 8

        let hashes_start = 8;
        let hashes_end = hashes_start + (count * 8);
        let offsets_start = hashes_end;
        let data_start = offsets_start + (count * 4);

        // 4. Fast Scan (SIMD Optimized via chunks_exact)
        let hashes_slice = payload
            .get(hashes_start..hashes_end)
            .ok_or_else(|| crate::ParcodeError::Format("Hashes slice out of bounds".into()))?;

        for (i, chunk) in hashes_slice.chunks_exact(8).enumerate() {
            let h = u64::from_le_bytes(
                chunk
                    .try_into()
                    .map_err(|_| crate::ParcodeError::Format("Failed to read hash".into()))?,
            );

            if h == target_hash {
                // Candidate found. Verify key to handle hash collisions.
                let offset_bytes = payload
                    .get(offsets_start + (i * 4)..offsets_start + (i * 4) + 4)
                    .ok_or_else(|| {
                        crate::ParcodeError::Format("Offset bytes out of bounds".into())
                    })?;
                let offset = u32::from_le_bytes(
                    offset_bytes
                        .try_into()
                        .map_err(|_| crate::ParcodeError::Format("Failed to read offset".into()))?,
                ) as usize;

                let data_slice = payload.get(data_start + offset..).ok_or_else(|| {
                    crate::ParcodeError::Format("Data slice out of bounds".into())
                })?;

                // Deserialize (K, V)
                // Use bincode::deserialize_from slice. Bincode knows when to stop.
                let (stored_key, stored_val): (K, V) =
                    bincode::serde::decode_from_slice(data_slice, bincode::config::standard())
                        .map_err(|e| crate::ParcodeError::Serialization(e.to_string()))?
                        .0;

                if &stored_key == key {
                    return Ok(Some(stored_val));
                }
                // If no match, it's a hash collision (rare). Continue searching.
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

    fn read_lazy_from_stream(
        _: &mut Cursor<&[u8]>,
        children: &mut IntoIter<ChunkNode<'a>>,
    ) -> Result<Self::Lazy> {
        let child_node = children.next().ok_or_else(|| {
            crate::ParcodeError::Format("Missing child node for HashMap field".into())
        })?;
        Ok(ParcodeMapPromise::new(child_node))
    }
}

impl<'a, K, V> ParcodeMapPromise<'a, K, V>
where
    K: Hash + Eq + DeserializeOwned + Send + Sync + 'static,
    V: ParcodeItem + Send + Sync + ParcodeLazyRef<'a> + 'a,
{
    /// Performs a lazy lookup for a single key.
    ///
    /// This returns a Lazy Mirror of the value, allowing access to its local fields
    /// without loading its heavy dependencies.
    pub fn get_lazy(&self, key: &K) -> Result<Option<V::Lazy>> {
        // 1. Read Container Payload (Num Shards)
        let container_payload = self.node.read_raw()?;
        if container_payload.len() < 4 {
            return Ok(None);
        } // Empty
        let num_shards = u32::from_le_bytes(
            container_payload
                .get(0..4)
                .ok_or_else(|| crate::ParcodeError::Format("Payload too short".into()))?
                .try_into()
                .map_err(|_| crate::ParcodeError::Format("Failed to read num_shards".into()))?,
        );

        // 2. Hash & Select Shard
        let target_hash = crate::map::hash_key(key);
        let shard_idx = usize::try_from(target_hash).unwrap_or(0) % (num_shards as usize);

        // 3. Load Shard
        let shard = self.node.get_child_by_index(shard_idx)?;
        let payload = shard.read_raw()?;

        if payload.len() < 8 {
            return Ok(None);
        } // Empty shard

        let count = u32::from_le_bytes(
            payload
                .get(0..4)
                .ok_or_else(|| crate::ParcodeError::Format("Payload too short for count".into()))?
                .try_into()
                .map_err(|_| crate::ParcodeError::Format("Failed to read count".into()))?,
        ) as usize;
        // Skip 4 bytes padding -> Offset 8

        let hashes_start = 8;
        let hashes_end = hashes_start + (count * 8);
        let offsets_start = hashes_end;
        let data_start = offsets_start + (count * 4);

        // 4. Fast Scan (SIMD Optimized via chunks_exact)
        let hashes_slice = payload
            .get(hashes_start..hashes_end)
            .ok_or_else(|| crate::ParcodeError::Format("Hashes slice out of bounds".into()))?;

        // We need to find the index `i` of the item.
        let mut target_index = None;

        for (i, chunk) in hashes_slice.chunks_exact(8).enumerate() {
            let h = u64::from_le_bytes(
                chunk
                    .try_into()
                    .map_err(|_| crate::ParcodeError::Format("Failed to read hash".into()))?,
            );

            if h == target_hash {
                // Candidate found. Verify key to handle hash collisions.
                let offset_bytes = payload
                    .get(offsets_start + (i * 4)..offsets_start + (i * 4) + 4)
                    .ok_or_else(|| {
                        crate::ParcodeError::Format("Offset bytes out of bounds".into())
                    })?;
                let offset = u32::from_le_bytes(
                    offset_bytes
                        .try_into()
                        .map_err(|_| crate::ParcodeError::Format("Failed to read offset".into()))?,
                ) as usize;

                let data_slice = payload.get(data_start + offset..).ok_or_else(|| {
                    crate::ParcodeError::Format("Data slice out of bounds".into())
                })?;

                // Deserialize K to check equality
                // We use decode_from_slice which returns (T, usize)
                let (stored_key, _): (K, usize) =
                    bincode::serde::decode_from_slice(data_slice, bincode::config::standard())
                        .map_err(|e| crate::ParcodeError::Serialization(e.to_string()))?;

                if &stored_key == key {
                    target_index = Some(i);
                    break;
                }
            }
        }

        let target_index = match target_index {
            Some(idx) => idx,
            None => return Ok(None),
        };

        // 5. We found the item at `target_index`.
        // Now we must advance the children iterator to the correct position.
        // This requires parsing items 0..target_index.

        let children = shard.children()?;
        let mut child_iter = children.into_iter();

        // Iterate 0..target_index
        for i in 0..target_index {
            let offset_bytes = payload
                .get(offsets_start + (i * 4)..offsets_start + (i * 4) + 4)
                .ok_or_else(|| crate::ParcodeError::Format("Offset bytes out of bounds".into()))?;
            let offset = u32::from_le_bytes(
                offset_bytes
                    .try_into()
                    .map_err(|_| crate::ParcodeError::Format("Failed to read offset".into()))?,
            ) as usize;

            let data_slice = payload
                .get(data_start + offset..)
                .ok_or_else(|| crate::ParcodeError::Format("Data slice out of bounds".into()))?;

            let mut cursor = Cursor::new(data_slice);

            // Read K
            let _: K =
                bincode::serde::decode_from_std_read(&mut cursor, bincode::config::standard())
                    .map_err(|e| crate::ParcodeError::Serialization(e.to_string()))?;

            // Read V (Lazy) - this consumes children
            let _ = V::read_lazy_from_stream(&mut cursor, &mut child_iter)?;
        }

        // 6. Read the target item
        let offset_bytes = payload
            .get(offsets_start + (target_index * 4)..offsets_start + (target_index * 4) + 4)
            .ok_or_else(|| crate::ParcodeError::Format("Offset bytes out of bounds".into()))?;
        let offset = u32::from_le_bytes(
            offset_bytes
                .try_into()
                .map_err(|_| crate::ParcodeError::Format("Failed to read offset".into()))?,
        ) as usize;

        let data_slice = payload
            .get(data_start + offset..)
            .ok_or_else(|| crate::ParcodeError::Format("Data slice out of bounds".into()))?;

        let mut cursor = Cursor::new(data_slice);

        // Read K (skip)
        let _: K = bincode::serde::decode_from_std_read(&mut cursor, bincode::config::standard())
            .map_err(|e| crate::ParcodeError::Serialization(e.to_string()))?;

        // Read V (Lazy) and return
        let lazy_val = V::read_lazy_from_stream(&mut cursor, &mut child_iter)?;

        Ok(Some(lazy_val))
    }
}

/// Streaming iterator for lazy collections.
///
/// This iterator efficiently traverses the shards of a vector, maintaining
/// a cursor position to avoid re-parsing previous items.
#[derive(Debug)]
pub struct ParcodeLazyIterator<'a, T: ParcodeLazyRef<'a>> {
    /// The shards (children of the container node).
    shards: std::vec::IntoIter<ChunkNode<'a>>,

    /// Decompressed payload of the active shard.
    current_payload: Option<Cow<'a, [u8]>>,
    /// Current position in the payload buffer (bytes).
    current_pos: u64,
    /// Iterator over the active shard's children (nested chunks).
    current_children: std::vec::IntoIter<ChunkNode<'a>>,
    /// How many items are left to read in this shard.
    items_left_in_shard: u64,

    total_items: usize,
    items_yielded: usize,

    _marker: PhantomData<T>,
}

impl<'a, T: ParcodeLazyRef<'a>> ParcodeLazyIterator<'a, T> {
    pub fn new(node: ChunkNode<'a>) -> Result<Self> {
        let total_items = usize::try_from(node.len()).unwrap_or(usize::MAX);
        let shards = node.children()?.into_iter();

        Ok(Self {
            shards,
            current_payload: None,
            current_pos: 0,
            current_children: Vec::new().into_iter(),
            items_left_in_shard: 0,
            total_items,
            items_yielded: 0,
            _marker: PhantomData,
        })
    }

    /// Advances to the next shard if the current one is exhausted.
    fn ensure_shard_loaded(&mut self) -> Result<bool> {
        if self.items_left_in_shard > 0 {
            return Ok(true);
        }

        // Current shard finished. Load next.
        if let Some(shard_node) = self.shards.next() {
            // 1. Load Payload
            let payload = shard_node.read_raw()?;

            // 2. Load Children
            let children = shard_node.children()?;

            // 3. Reset State
            // Read length prefix (standard for Parcode slice serialization)
            let mut cursor = Cursor::new(payload.as_ref());
            let len: u64 =
                bincode::serde::decode_from_std_read(&mut cursor, bincode::config::standard())
                    .map_err(|e| crate::ParcodeError::Serialization(e.to_string()))?;

            self.current_pos = cursor.position();
            self.items_left_in_shard = len;
            self.current_payload = Some(payload);
            self.current_children = children.into_iter();

            Ok(true)
        } else {
            Ok(false) // No more shards
        }
    }
}

impl<'a, T: ParcodeLazyRef<'a>> Iterator for ParcodeLazyIterator<'a, T> {
    type Item = Result<T::Lazy>;

    fn next(&mut self) -> Option<Self::Item> {
        // 1. Ensure we have data
        match self.ensure_shard_loaded() {
            Ok(true) => {}
            Ok(false) => return None, // End of iteration
            Err(e) => return Some(Err(e)),
        }

        // 2. Prepare Cursor at current position
        let payload = self
            .current_payload
            .as_ref()
            .expect("ensure_shard_loaded guaranteed payload");
        let mut cursor = Cursor::new(payload.as_ref());
        cursor.set_position(self.current_pos);

        // 3. Deserialize ONE item (Lazy)
        // This advances the cursor and the children iterator
        let result = T::read_lazy_from_stream(&mut cursor, &mut self.current_children);

        // 4. Update State
        self.current_pos = cursor.position();
        self.items_left_in_shard -= 1;
        self.items_yielded += 1;

        Some(result)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.total_items - self.items_yielded;
        (remaining, Some(remaining))
    }
}

impl<'a, T: ParcodeLazyRef<'a>> ExactSizeIterator for ParcodeLazyIterator<'a, T> {
    fn len(&self) -> usize {
        self.total_items - self.items_yielded
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

                fn read_lazy_from_stream(
                    _: &mut Cursor<&[u8]>,
                    children: &mut IntoIter<ChunkNode<'a>>,
                ) -> Result<Self::Lazy> {
                    let child_node = children.next().ok_or_else(|| {
                        crate::ParcodeError::Format("Missing child node for chunkable primitive field".into())
                    })?;
                    Ok(ParcodePromise::new(child_node))
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

    fn read_lazy_from_stream(
        _: &mut Cursor<&[u8]>,
        children: &mut IntoIter<ChunkNode<'a>>,
    ) -> Result<Self::Lazy> {
        let child_node = children.next().ok_or_else(|| {
            crate::ParcodeError::Format("Missing child node for Vec field".into())
        })?;

        Ok(ParcodeCollectionPromise::new(child_node))
    }
}
