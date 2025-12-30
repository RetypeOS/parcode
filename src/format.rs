//! Defines the physical binary layout of Parcode V4 files.
//!
//! This module specifies the on-disk representation of Parcode files, including the
//! global header, chunk structure, and metadata encoding. Understanding this format
//! is essential for implementing readers in other languages or debugging file corruption.
//!
//! ## File Format Overview (V4)
//!
//! Parcode V4 uses a "bottom-up" layout strategy where children are written before their
//! parents. This enables streaming writes and allows the root chunk to contain a complete
//! table of contents for the entire file.
//!
//! ### High-Level Structure
//!
//! ```text
//! ┌──────────────────────────────────┐
//! │ Chunk 0 (Leaf)                   │
//! ├──────────────────────────────────┤
//! │ Chunk 1 (Leaf)                   │
//! ├──────────────────────────────────┤
//! │ ...                              │
//! ├──────────────────────────────────┤
//! │ Chunk N (Parent)                 │
//! ├──────────────────────────────────┤
//! │ Root Chunk                       │
//! ├──────────────────────────────────┤
//! │ Global Header (26 bytes)         │
//! └──────────────────────────────────┘
//! ```
//!
//! ## Chunk Anatomy
//!
//! Each chunk is self-contained and consists of three parts:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │ Compressed Payload (Variable Length)                    │
//! │   - Contains the actual data (serialized with bincode)  │
//! │   - May be compressed (LZ4, etc.) based on MetaByte     │
//! ├─────────────────────────────────────────────────────────┤
//! │ Children Table (Optional, only if is_chunkable = true)  │
//! │   - Array of ChildRef structures (16 bytes each)        │
//! │   - Count stored as u32 LE (4 bytes) at the end         │
//! ├─────────────────────────────────────────────────────────┤
//! │ MetaByte (1 byte)                                       │
//! │   - Bit 0: is_chunkable (has children)                  │
//! │   - Bits 1-3: compression_method (0-7)                  │
//! │   - Bits 4-7: Reserved for future use                   │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! ### Reading a Chunk
//!
//! To read a chunk at offset `O` with length `L`:
//!
//! 1. Read the `MetaByte` at `O + L - 1`
//! 2. If `is_chunkable`, read the child count at `O + L - 5` (u32 LE)
//! 3. Read the children table (if present) working backwards from the count
//! 4. The payload starts at `O` and ends before the children table (or `MetaByte` if no children)
//! 5. Decompress the payload based on the compression method
//!
//! ## Global Header
//!
//! The global header is always located at the end of the file and has a fixed size of 26 bytes:
//!
//! ```text
//! Offset | Size | Field         | Description
//! -------|------|---------------|----------------------------------------
//! 0      | 4    | magic         | Magic bytes: "PAR4" (0x50 0x41 0x52 0x34)
//! 4      | 2    | version       | Format version (u16 LE, currently 4)
//! 6      | 8    | root_offset   | Absolute offset of root chunk (u64 LE)
//! 14     | 8    | root_length   | Total length of root chunk (u64 LE)
//! 22     | 4    | checksum      | Reserved for CRC32 (u32 LE, currently 0)
//! ```
//!
//! ## Design Rationale
//!
//! ### Why Bottom-Up Layout?
//!
//! - **Streaming Writes:** Children can be written as soon as they're ready, without
//!   knowing the final file size
//! - **Parallel Execution:** Multiple threads can write chunks concurrently without
//!   coordination beyond the sequential writer mutex
//! - **Self-Describing Root:** The root chunk contains all necessary metadata to
//!   navigate the entire file
//!
//! ### Why `MetaByte` at the End?
//!
//! - **Backward Reading:** When navigating from a parent to children, we can read
//!   the `MetaByte` first to determine the chunk structure
//! - **Alignment:** Placing metadata at the end avoids alignment issues with the payload
//!
//! ### Why Fixed-Size `ChildRef`?
//!
//! - **Random Access:** Fixed-size references enable O(1) indexing into the children array
//! - **Simplicity:** No need for variable-length encoding or delimiters
//!
//! ## Compatibility
//!
//! - **Endianness:** All multi-byte integers use little-endian encoding
//! - **Alignment:** No special alignment requirements (can be read from any offset)
//! - **Version Detection:** Readers should check the magic bytes and version before parsing

use crate::error::{ParcodeError, Result};

/// Magic bytes identifying the file format: "PAR4".
pub const MAGIC_BYTES: [u8; 4] = *b"PAR4";

/// The fixed size of the Global Header.
/// Magic(4) + Version(2) + RootOffset(8) + RootLength(8) + Checksum(4) = 26
pub const GLOBAL_HEADER_SIZE: usize = 26;

/// Configuration flags for a specific chunk, stored in the last byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetaByte(u8);

impl MetaByte {
    const CHUNKABLE_MASK: u8 = 0b0000_0001; // Bit 0
    const COMPRESSION_MASK: u8 = 0b0000_1110; // Bits 1-3

    /// Creates a new `MetaByte`.
    pub fn new(is_chunkable: bool, compression_id: u8) -> Self {
        let mut byte = 0;
        if is_chunkable {
            byte |= Self::CHUNKABLE_MASK;
        }
        // Compress ID lives in bits 1-3
        byte |= (compression_id & 0x07) << 1;
        Self(byte)
    }

    /// decodes the byte.
    pub fn from_byte(byte: u8) -> Self {
        Self(byte)
    }

    /// Returns true if the chunk contains references to children.
    pub fn is_chunkable(&self) -> bool {
        (self.0 & Self::CHUNKABLE_MASK) != 0
    }

    /// Returns the compression algorithm ID (0-7).
    pub fn compression_method(&self) -> u8 {
        (self.0 & Self::COMPRESSION_MASK) >> 1
    }

    /// Returns the raw byte representation.
    pub fn as_u8(&self) -> u8 {
        self.0
    }
}

/// Represents a reference to a child chunk stored within a parent chunk.
/// This allows the reader to locate dependencies without deserializing the payload.
#[derive(Debug, Clone, Copy)]
pub struct ChildRef {
    /// Absolute offset in the file where the child chunk starts.
    pub offset: u64,
    /// Total length of the child chunk (including meta-byte).
    pub length: u64,
}

impl ChildRef {
    /// The size in bytes of a serialized `ChildRef`.
    pub const SIZE: usize = 16; // 8 bytes offset + 8 bytes length

    /// Serializes to a fixed-size byte array (Little Endian).
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        buf[0..8].copy_from_slice(&self.offset.to_le_bytes());
        buf[8..16].copy_from_slice(&self.length.to_le_bytes());
        buf
    }

    /// Deserializes from a fixed-size byte array.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < Self::SIZE {
            return Err(ParcodeError::Format("Buffer too small for ChildRef".into()));
        }

        let offset_bytes = bytes.get(0..8).ok_or_else(|| {
            ParcodeError::Format("Failed to read offset from ChildRef buffer".into())
        })?;
        let length_bytes = bytes.get(8..16).ok_or_else(|| {
            ParcodeError::Format("Failed to read length from ChildRef buffer".into())
        })?;

        let offset = u64::from_le_bytes(
            offset_bytes
                .try_into()
                .map_err(|_| ParcodeError::Format("Invalid offset bytes".into()))?,
        );
        let length = u64::from_le_bytes(
            length_bytes
                .try_into()
                .map_err(|_| ParcodeError::Format("Invalid length bytes".into()))?,
        );

        Ok(Self { offset, length })
    }
}

/// The Global Header located at the very end of the file (Tail).
/// It points to the Root Chunk, which is the entry point for the graph.
#[derive(Debug, Clone, Copy)]
pub struct GlobalHeader {
    /// The magic bytes identifying the file format.
    pub magic: [u8; 4],
    /// The version of the file format (currently 4).
    pub version: u16,
    /// Pointer to the final Root Chunk.
    pub root_offset: u64,
    /// The total length of the Root Chunk.
    pub root_length: u64,
    /// Reserved for CRC/Checksum of the header itself.
    pub checksum: u32,
}

impl GlobalHeader {
    /// Creates a new `GlobalHeader`.
    pub fn new(root_offset: u64, root_length: u64) -> Self {
        Self {
            magic: MAGIC_BYTES,
            version: 4,
            root_offset,
            root_length,
            checksum: 0,
        }
    }

    /// Serializes the header to bytes.
    pub fn to_bytes(&self) -> [u8; GLOBAL_HEADER_SIZE] {
        let mut buf = [0u8; GLOBAL_HEADER_SIZE];
        buf[0..4].copy_from_slice(&self.magic);
        buf[4..6].copy_from_slice(&self.version.to_le_bytes());
        buf[6..14].copy_from_slice(&self.root_offset.to_le_bytes());
        buf[14..22].copy_from_slice(&self.root_length.to_le_bytes());
        buf[22..26].copy_from_slice(&self.checksum.to_le_bytes());
        buf
    }
}
