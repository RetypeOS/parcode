//! Example: Writing Parcode to a Memory Buffer
//!
//! This example demonstrates how to use the generic `write_to_writer` API
//! to serialize a Parcode object directly into a `Vec<u8>` in memory,
//! bypassing the file system.

#![allow(missing_docs)]

use parcode::{Parcode, ParcodeObject};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, ParcodeObject, Debug, PartialEq)]
struct UserProfile {
    id: u64,
    username: String,
    tags: Vec<String>,
    scores: Vec<u32>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("--- Parcode Memory Buffer Example ---");

    // 1. Create some data
    let user = UserProfile {
        id: 12345,
        username: "generic_writer_fan".to_string(),
        tags: vec!["rust".into(), "parcode".into(), "memory".into()],
        scores: vec![100, 200, 300, 400, 500],
    };

    println!("Original data: {:?}", user);

    // 2. Prepare a memory buffer (Vec<u8>)
    // Parcode will write into this vector.
    // Note: The vector will grow as needed.
    let mut buffer: Vec<u8> = Vec::new();

    // 3. Serialize to the buffer
    // We use `write_to_writer` which accepts any W: Write + Send
    // Vec<u8> implements Write and Send.
    println!("Serializing to memory buffer...");
    Parcode::builder()
        .compression(true) // Optional: enable compression
        .write_to_writer(&mut buffer, &user)?;

    println!(
        "Serialization complete. Buffer size: {} bytes",
        buffer.len()
    );

    // 4. Verify the buffer content (Forensics)
    // The last 26 bytes should be the Global Header.
    // Let's print the first few bytes (Magic) and the last few.
    if buffer.len() > 30 {
        println!(
            "Magic bytes: {:02X?}",
            &buffer.get(0..8).expect("Failed to get magic bytes")
        );
        println!(
            "Tail bytes:  {:02X?}",
            &buffer
                .get(buffer.len() - 26..)
                .expect("Failed to get tail bytes")
        );
    }

    // 5. (Optional) Write buffer to disk to verify with standard reader
    // In a real app, you might send this buffer over network, store in DB, etc.
    let path = "memory_dump.par";
    std::fs::write(path, &buffer)?;
    println!("Dumped buffer to '{}' for verification.", path);

    // 6. Read it back using standard Parcode::read
    // Note: Currently Parcode::read expects a file path because it uses memory mapping.
    // Reading from a memory buffer directly (without file) would require a `ParcodeReader::from_bytes`
    // which is a separate feature (Reader refactor).
    // For now, we verify by reading the dumped file.
    let loaded: UserProfile = Parcode::read(path)?;
    println!("Loaded data:   {:?}", loaded);

    assert_eq!(user, loaded);
    println!("Success! Data matches.");

    // Cleanup
    std::fs::remove_file(path)?;

    Ok(())
}
