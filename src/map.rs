//! PLACEHOLDER

use crate::error::{ParcodeError, Result};
use crate::format::ChildRef;
use crate::graph::SerializationJob;
use serde::Serialize;
use std::hash::{Hash, Hasher};
use twox_hash::XxHash64;

pub(crate) fn hash_key<K: Hash>(key: &K) -> u64 {
    let mut hasher = XxHash64::with_seed(0);
    key.hash(&mut hasher);
    hasher.finish()
}

/// Job for a single Hash Shard.
/// Layout: [Count(4)] [Padding(4)] [Hashes(8*N)] [Offsets(4*N)] [DataBlob]
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

        let mut data_blob = Vec::new();
        let mut offsets = Vec::with_capacity(count * 4);
        let mut hashes = Vec::with_capacity(count * 8);

        let mut cursor = std::io::Cursor::new(&mut data_blob);

        for (k, v) in &self.items {
            // 1. Hash
            hashes.extend_from_slice(&hash_key(k).to_le_bytes());

            // 2. Offset (relative to data start)
            let pos = cursor.position() as u32;
            offsets.extend_from_slice(&pos.to_le_bytes());

            // 3. Data (Key + Value for collision check)
            bincode::serde::encode_into_std_write(
                &(k, v),
                &mut cursor,
                bincode::config::standard(),
            )
            .map_err(|e| ParcodeError::Serialization(e.to_string()))?;
        }

        // 4. Assembly with Alignment
        // Header: Count (4 bytes).
        // Alignment target for Hashes is 8 bytes.
        // Current size: 4. Padding needed: 4.

        let hashes_size = count * 8;
        let offsets_size = count * 4;
        let total_size = 8 + hashes_size + offsets_size + data_blob.len();

        let mut final_buf = Vec::with_capacity(total_size);

        // Write Count
        final_buf.extend_from_slice(&(count as u32).to_le_bytes());

        // Write Padding (4 bytes of zeros)
        final_buf.extend_from_slice(&[0u8; 4]);

        // Write Hashes (Aligned at offset 8)
        final_buf.extend_from_slice(&hashes);

        // Write Offsets
        final_buf.extend_from_slice(&offsets);

        // Write Data
        final_buf.extend_from_slice(&data_blob);

        Ok(final_buf)
    }

    fn estimated_size(&self) -> usize {
        // Crude estimation
        self.items.len() * (std::mem::size_of::<K>() + std::mem::size_of::<V>() + 12)
    }
}
