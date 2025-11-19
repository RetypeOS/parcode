//! The Read-Side Engine.
//!
//! Handles memory-mapping the file, validating the global structure,
//! and providing random access to chunks via the dependency graph.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::borrow::Cow;
use memmap2::Mmap;

use crate::error::{ParcodeError, Result};
use crate::format::{GlobalHeader, MetaByte, ChildRef, GLOBAL_HEADER_SIZE, MAGIC_BYTES};
use crate::compression::{self, Compressor};

/// The main handle for reading a Parcode file.
/// It holds the memory map and the global header.
#[derive(Debug)]
pub struct ParcodeReader {
    mmap: Arc<Mmap>,
    header: GlobalHeader,
    file_size: u64,
}

/// A view into a specific chunk within the file.
/// This is a lightweight handle that doesn't own data, just points to it.
#[derive(Debug, Clone)]
pub struct ChunkNode<'a> {
    reader: &'a ParcodeReader,
    offset: u64,
    length: u64,
    meta: MetaByte,
    child_count: u32,
    payload_end_offset: u64,
}

impl ParcodeReader {
    /// Opens a Parcode file and validates its integrity.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path)?;
        let file_size = file.metadata()?.len();

        if file_size < GLOBAL_HEADER_SIZE as u64 {
            return Err(ParcodeError::Format("File smaller than header".into()));
        }

        // Safety: Mmap is fundamentally unsafe as external processes could modify the file.
        // We assume exclusive access or accept the risk for performance (standard practice).
        #[allow(unsafe_code)]
        let mmap = unsafe { Mmap::map(&file)? };

        // Read Global Header (Last 24 bytes)
        let header_start = (file_size as usize) - GLOBAL_HEADER_SIZE;
        let header_bytes = &mmap[header_start..];
        
        // Manual parsing of header
        if header_bytes[0..4] != MAGIC_BYTES {
             return Err(ParcodeError::Format("Invalid Magic Bytes".into()));
        }
        
        // Extract fields (simplified for brevity, normally we use helper)
        let version = u16::from_le_bytes(header_bytes[4..6].try_into().unwrap());
        if version != 4 {
             return Err(ParcodeError::Format(format!("Unsupported version: {version}")));
        }

        let root_offset = u64::from_le_bytes(header_bytes[6..14].try_into().unwrap());
        let root_length = u64::from_le_bytes(header_bytes[14..22].try_into().unwrap());
        let checksum = u32::from_le_bytes(header_bytes[22..26].try_into().unwrap());

        Ok(Self {
            mmap: Arc::new(mmap),
            header: GlobalHeader {
                magic: MAGIC_BYTES,
                version,
                root_offset,
                root_length,
                checksum,
            },
            file_size,
        })
    }

    /// Returns a view of the Root Chunk.
    pub fn root(&self) -> Result<ChunkNode<'_>> {
        self.get_chunk(self.header.root_offset, self.header.root_length)
    }

    /// Retrieves any chunk by its physical reference.
    pub fn get_chunk(&self, offset: u64, length: u64) -> Result<ChunkNode<'_>> {
        // Bounds check
        if offset + length > self.file_size {
            return Err(ParcodeError::Format("Chunk out of file bounds".into()));
        }

        let chunk_end = (offset + length) as usize;
        
        // 1. Read MetaByte (Last byte)
        let meta_byte_val = self.mmap[chunk_end - 1];
        let meta = MetaByte::from_byte(meta_byte_val);

        // 2. Parse Footer Layout
        // Layout: [Payload] ... [ChildRef * N] [u32 Count] [MetaByte]
        
        let mut child_count = 0;
        let mut payload_end = chunk_end - 1; // Start assuming no children

        if meta.is_chunkable() {
            // Read u32 ChildCount (4 bytes before MetaByte)
            if length < 5 { // 1 meta + 4 count
                return Err(ParcodeError::Format("Chunk too small for metadata".into()));
            }
            
            let count_start = chunk_end - 1 - 4;
            let count_bytes = &self.mmap[count_start..count_start + 4];
            child_count = u32::from_le_bytes(count_bytes.try_into().unwrap());
            
            // Calculate payload end
            let footer_size = child_count as usize * ChildRef::SIZE;
            let total_metadata_size = 1 + 4 + footer_size; // Meta + Count + Refs
            
            if length < total_metadata_size as u64 {
                return Err(ParcodeError::Format("Invalid child count for chunk size".into()));
            }

            payload_end = chunk_end - total_metadata_size;
        }

        Ok(ChunkNode {
            reader: self,
            offset,
            length,
            meta,
            child_count,
            payload_end_offset: offset + (payload_end as u64 - offset),
        })
    }
}

impl<'a> ChunkNode<'a> {

    /// Returns the total length of the chunk on disk.
    pub fn len(&self) -> u64 { self.length }

    /// Decompresses and returns the data payload.
    /// This payload DOES NOT contain the children data, only the local struct data.
    pub fn read_payload(&self) -> Result<Cow<'a, [u8]>> {
        let start = self.offset as usize;
        let end = self.payload_end_offset as usize;
        let raw_bytes = &self.reader.mmap[start..end];

        let algo_id = self.meta.compression_method();
        
        // Select decompressor based on ID
        match algo_id {
            0 => compression::NoCompression.decompress(raw_bytes).map(Cow::Owned),
            #[cfg(feature = "lz4_flex")]
            1 => compression::Lz4Compressor.decompress(raw_bytes).map(Cow::Owned),
            _ => Err(ParcodeError::Compression(format!("Unknown algo id: {algo_id}"))),
        }
        // Note: For Cow::Borrowed optimization in NoCompression, we'd need to change
        // the Compressor trait signature to allow returning Cow. For now, Vec<u8> is safe.
    }

    /// Returns the list of references to children chunks.
    /// This operation is Zero-Copy (allocates only the vector of structs).
    pub fn children(&self) -> Result<Vec<ChunkNode<'a>>> {
        if self.child_count == 0 {
            return Ok(Vec::new());
        }

        let mut children = Vec::with_capacity(self.child_count as usize);
        
        // Footer starts after payload
        let footer_start = self.payload_end_offset as usize;
        
        for i in 0..self.child_count {
            let entry_start = footer_start + (i as usize * ChildRef::SIZE);
            let entry_bytes = &self.reader.mmap[entry_start..entry_start + ChildRef::SIZE];
            
            let child_ref = ChildRef::from_bytes(entry_bytes)?;
            
            // Recursively get the view for the child
            children.push(self.reader.get_chunk(child_ref.offset, child_ref.length)?);
        }
        Ok(children)
    }

    /// O(1) Random Access to a specific child without parsing the whole table.
    pub fn get_child(&self, index: usize) -> Result<ChunkNode<'a>> {
        if index >= self.child_count as usize {
             return Err(ParcodeError::Format("Index out of bounds".into()));
        }

        // Footer start calculation
        let footer_start = self.payload_end_offset as usize;
        
        // Calculate exact byte offset for this specific child reference
        let entry_start = footer_start + (index * ChildRef::SIZE);
        let entry_bytes = &self.reader.mmap[entry_start..entry_start + ChildRef::SIZE];
        
        let child_ref = ChildRef::from_bytes(entry_bytes)?;
        self.reader.get_chunk(child_ref.offset, child_ref.length)
    }

    /// Deserializes the payload into a Rust struct.
    /// Note: This only fills the fields stored in this chunk. 
    /// Complex fields (Vectors/Maps) needing children data must be handled by 
    /// the caller or a custom reconstruction utility using `children()`.
    pub fn deserialize_local<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        let payload = self.read_payload()?;
        bincode::serde::decode_from_slice(&payload, bincode::config::standard())
            .map(|(obj, _)| obj)
            .map_err(|e| ParcodeError::Serialization(e.to_string()))
    }
}