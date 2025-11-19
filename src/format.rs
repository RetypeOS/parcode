//! Defines the physical binary layout of Parcode V4 files.
//!
//! # V4 Layout Strategy
//! The file consists of a sequence of Chunks written in a bottom-up order (children first),
//! followed by a Global Header at the very end of the file.
//!
//! File: `[Chunk 0] [Chunk 1] ... [Root Chunk] [Global Header]`
//!
//! ## Chunk Anatomy
//! Each chunk is self-contained:
//! `[ Compressed Payload ] [ Children Table (Optional) ] [ MetaByte ]`

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
    
    /// Creates a new MetaByte.
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
    /// The size in bytes of a serialized ChildRef.
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
        let offset = u64::from_le_bytes(bytes[0..8].try_into().unwrap_or([0; 8]));
        let length = u64::from_le_bytes(bytes[8..16].try_into().unwrap_or([0; 8]));
        Ok(Self { offset, length })
    }
}

/// The Global Header located at the very end of the file (Tail).
/// It points to the Root Chunk, which is the entry point for the graph.
#[derive(Debug, Clone, Copy)]
pub struct GlobalHeader {
    /// PLACEHOLDER
    pub magic: [u8; 4],
    /// PLACEHOLDER
    pub version: u16,
    /// Pointer to the final Root Chunk.
    pub root_offset: u64,
    /// PLACEHOLDER
    pub root_length: u64,
    /// Reserved for CRC/Checksum of the header itself.
    pub checksum: u32, 
}

impl GlobalHeader {
    /// Creates a new GlobalHeader.
    pub fn new(root_offset: u64, root_length: u64) -> Self {
        Self {
            magic: MAGIC_BYTES,
            version: 4,
            root_offset,
            root_length,
            checksum: 0, // To be calculated if needed
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