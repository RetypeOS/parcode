// src/inspector.rs

//! Tools for inspecting the physical structure of Parcode files.
//! useful for debugging sharding strategies and verification.

use crate::error::Result;
use crate::reader::{ChunkNode, ParcodeReader};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A structural report of a Parcode file.
#[derive(Debug, Serialize)]
pub struct DebugReport {
    /// Total size of the file on disk.
    pub file_size: u64,
    /// Offset where the Root chunk starts.
    pub root_offset: u64,
    /// Format version.
    pub global_version: u16,
    /// The hierarchical tree of chunks.
    pub tree: ChunkInfo,
}

/// Metadata for a single chunk in the graph.
#[derive(Debug, Serialize)]
pub struct ChunkInfo {
    /// Absolute offset.
    pub offset: u64,
    /// Total chunk length (header + payload + footer).
    pub total_length: u64,
    /// Size of the data payload.
    pub payload_size: u64,
    /// Number of children dependencies.
    pub child_count: u32,
    /// Compression algorithm used.
    pub compression_algo: String,
    /// Whether it flagged as having children.
    pub is_chunkable: bool,
    /// Inferred type info (e.g., "Vec Container", "Map Shard", "Blob").
    pub node_type_hint: String,
    /// Extra info about distribution (e.g., "32 shards").
    pub distribution_info: Option<String>,
    /// Child nodes.
    pub children: Vec<ChunkInfo>,
}

/// The Parcode Inspector tool.
#[derive(Debug)]
pub struct ParcodeInspector;

impl ParcodeInspector {
    /// Analyzes a file and returns a structural report.
    pub fn inspect<P: AsRef<Path>>(path: P) -> Result<DebugReport> {
        let reader = ParcodeReader::open(path)?;
        let root = reader.root()?;

        // Note: In a real prod scenario, we might want to expose file_size via reader getter
        // For now we trust the reader opened successfully.

        let tree = Self::inspect_node(&root)?;

        Ok(DebugReport {
            file_size: 0, // Placeholder or need to expose reader.file_size
            root_offset: root.offset(),
            global_version: 4,
            tree,
        })
    }

    fn inspect_node(node: &ChunkNode<'_>) -> Result<ChunkInfo> {
        let raw_meta = node.meta();
        let algo_id = raw_meta.compression_method();
        let algo_name = match algo_id {
            0 => "None".to_string(),
            1 => "LZ4".to_string(),
            _ => format!("Unknown({})", algo_id),
        };

        let (hint, distrib) = Self::analyze_payload(node);

        let mut children_info = Vec::new();
        if node.child_count() > 0 {
            let children = node.children()?;
            for child in children {
                children_info.push(Self::inspect_node(&child)?);
            }
        }

        Ok(ChunkInfo {
            offset: node.offset(),
            total_length: node.length(),
            payload_size: node.payload_len(),
            child_count: node.child_count(),
            compression_algo: algo_name,
            is_chunkable: raw_meta.is_chunkable(),
            node_type_hint: hint,
            distribution_info: distrib,
            children: children_info,
        })
    }

    fn analyze_payload(node: &ChunkNode<'_>) -> (String, Option<String>) {
        if node.child_count() == 0 {
            return ("Leaf/Blob".to_string(), None);
        }

        let payload = match node.read_raw() {
            Ok(p) => p,
            Err(_) => return ("Corrupted".to_string(), None),
        };

        // CHECK 1: Vec Container
        if payload.len() >= 8 {
            #[derive(Deserialize)]
            struct ShardRun {
                _item_count: u32,
                repeat: u32,
            }

            let slice = &payload.get(8..).expect("Missing Vec header");
            // We attempt to decode. If it fails, it's not a Vec header.
            if let Ok((runs, _)) = bincode::serde::decode_from_slice::<Vec<ShardRun>, _>(
                slice,
                bincode::config::standard(),
            ) {
                let total_items = if payload.len() >= 8 {
                    u64::from_le_bytes(
                        payload
                            .get(0..8)
                            .expect("Missing Vec header")
                            .try_into()
                            .unwrap_or([0; 8]),
                    )
                } else {
                    0
                };

                let distribution = format!(
                    "Vec<{}> items across {} logical shards",
                    total_items,
                    runs.iter().map(|r| r.repeat).sum::<u32>()
                );
                return ("Vec Container".to_string(), Some(distribution));
            }
        }

        // CHECK 2: Map Container (4 bytes exactly)
        if payload.len() == 4 {
            let num_shards = u32::from_le_bytes(
                payload
                    .get(0..4)
                    .expect("Missing Map header")
                    .try_into()
                    .unwrap_or([0; 4]),
            );
            if num_shards == node.child_count() {
                return (
                    "Map Container".to_string(),
                    Some(format!("Hashtable with {} buckets", num_shards)),
                );
            }
        }

        ("Generic Container".to_string(), None)
    }
}

impl std::fmt::Display for DebugReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "=== PARCODE INSPECTOR REPORT ===")?;
        writeln!(f, "Root Offset:    {}", self.root_offset)?;
        writeln!(f, "\n[GRAPH LAYOUT]")?;
        self.tree.fmt_recursive(f, "", true)
    }
}

impl ChunkInfo {
    fn fmt_recursive(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        prefix: &str,
        is_last: bool,
    ) -> std::fmt::Result {
        let connector = if is_last { "└── " } else { "├── " };
        let child_prefix = if is_last { "    " } else { "│   " };
        let distrib = self
            .distribution_info
            .as_deref()
            .map(|d| format!(" [{}]", d))
            .unwrap_or_default();

        writeln!(
            f,
            "{}{}[{}] Size: {}b | Algo: {} | Children: {}{}",
            prefix,
            connector,
            self.node_type_hint,
            self.payload_size,
            self.compression_algo,
            self.child_count,
            distrib
        )?;

        for (i, child) in self.children.iter().enumerate() {
            let is_last_child = i == self.children.len() - 1;
            child.fmt_recursive(f, &format!("{}{}", prefix, child_prefix), is_last_child)?;
        }
        Ok(())
    }
}
