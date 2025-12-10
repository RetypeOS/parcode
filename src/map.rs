/// HashMap Optimization Strategies.
///
/// This module implements efficient serialization and lookup strategies for `HashMap` types.
/// It uses hash-based sharding to distribute entries across multiple chunks, enabling:
///
/// * **Fast Lookups:** O(1) shard selection via hash modulo, followed by linear scan within shard
/// * **Parallel Serialization:** Independent shards can be compressed concurrently
/// * **Cache-Friendly Layout:** SOA (Structure of Arrays) format with aligned hash arrays
///
/// # Shard Layout
///
/// Each shard uses a Structure-of-Arrays format optimized for SIMD scanning:
/// ```text
/// [Count(4)] [Padding(4)] [Hashes(8*N)] [Offsets(4*N)] [DataBlob]
/// ```
///
/// The padding ensures 8-byte alignment for the hash array, enabling efficient SIMD operations.
use crate::error::{ParcodeError, Result};
use crate::format::ChildRef;
use crate::graph::SerializationJob;
use serde::Serialize;
use std::hash::{Hash, Hasher};
use twox_hash::XxHash64;

/// Computes a 64-bit hash of a key using the xxHash algorithm.
///
/// This hash function is used to:
/// 1. Determine which shard a key-value pair belongs to (via modulo)
/// 2. Enable fast lookups by storing the hash alongside the data
///
/// # Arguments
/// * `key` - The key to hash
///
/// # Returns
/// A 64-bit hash value
pub(crate) fn hash_key<K: Hash>(key: &K) -> u64 {
    let mut hasher = XxHash64::with_seed(0);
    key.hash(&mut hasher);
    hasher.finish()
}

/// Serialization job for a single HashMap shard.
///
/// This job serializes a subset of the HashMap's entries using a Structure-of-Arrays (SOA) layout.
/// The layout is optimized for:
/// - **SIMD scanning** during lookups (aligned hash array)
/// - **Cache efficiency** (sequential memory access patterns)
/// - **Collision resolution** (stores both key and value for verification)
///
/// # Layout
/// ```text
/// [Count(4)] [Padding(4)] [Hashes(8*N)] [Offsets(4*N)] [DataBlob]
/// ```
///
/// Where:
/// - `Count`: Number of entries in this shard (u32 LE)
/// - `Padding`: 4 bytes of zeros to ensure 8-byte alignment for hashes
/// - `Hashes`: Array of 64-bit hashes (u64 LE), one per entry
/// - `Offsets`: Array of byte offsets into DataBlob (u32 LE), one per entry
/// - `DataBlob`: Concatenated bincode-serialized (K, V) tuples
#[derive(Debug)]
pub struct MapShardJob<'a, K, V> {
    /// Items to be serialized in this shard.
    pub items: Vec<(&'a K, &'a V)>,
}

impl<'a, K, V> SerializationJob<'a> for MapShardJob<'a, K, V>
where
    K: Serialize + Hash + Sync,
    V: Serialize + Sync,
{
    fn execute(&self, _: &[ChildRef]) -> Result<Vec<u8>> {
        let count = self.items.len();
        if count == 0 {
            return Ok(Vec::new());
        }

        // Temporary buffers for SOA components
        let mut data_blob = Vec::new();
        let mut offsets = Vec::with_capacity(count * 4);
        let mut hashes = Vec::with_capacity(count * 8);

        let mut cursor = std::io::Cursor::new(&mut data_blob);

        for (k, v) in &self.items {
            // 1. Compute and store hash (for fast lookup)
            hashes.extend_from_slice(&hash_key(k).to_le_bytes());

            // 2. Record current offset in data blob (for random access)
            let pos = cursor.position() as u32;
            offsets.extend_from_slice(&pos.to_le_bytes());

            // 3. Serialize (K, V) tuple to data blob
            //    Both key and value are stored to enable collision resolution
            bincode::serde::encode_into_std_write(
                &(k, v),
                &mut cursor,
                bincode::config::standard(),
            )
            .map_err(|e| ParcodeError::Serialization(e.to_string()))?;
        }

        // 4. Assemble final buffer with proper alignment
        // Header: Count (4 bytes)
        // Alignment target for Hashes is 8 bytes
        // Current size: 4. Padding needed: 4

        let hashes_size = count * 8;
        let offsets_size = count * 4;
        let total_size = 8 + hashes_size + offsets_size + data_blob.len();

        let mut final_buf = Vec::with_capacity(total_size);

        // Write Count (u32 LE)
        final_buf.extend_from_slice(&(count as u32).to_le_bytes());

        // Write Padding (4 bytes of zeros for 8-byte alignment)
        final_buf.extend_from_slice(&[0u8; 4]);

        // Write Hashes (Aligned at offset 8)
        final_buf.extend_from_slice(&hashes);

        // Write Offsets
        final_buf.extend_from_slice(&offsets);

        // Write Data Blob
        final_buf.extend_from_slice(&data_blob);

        Ok(final_buf)
    }

    fn estimated_size(&self) -> usize {
        // Crude estimation: per-item overhead (hash + offset + padding) + data size
        self.items.len() * (std::mem::size_of::<K>() + std::mem::size_of::<V>() + 12)
    }
}
