//! Verification test for .`get_lazy()` on `HashMaps`.

use parcode::{Parcode, ParcodeObject, ParcodeReader};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A user profile with a heavy bio field.
#[derive(Serialize, Deserialize, ParcodeObject, Debug, Clone)]
struct UserProfile {
    /// User ID.
    id: u64,
    /// User name.
    name: String,
    /// User biography, stored in a separate chunk.
    #[parcode(chunkable)]
    bio: String, // Heavy field
}

/// A database of users.
#[derive(Serialize, Deserialize, ParcodeObject, Debug)]
struct UserDatabase {
    /// Map of users, sharded for O(1) access.
    #[parcode(map)]
    users: HashMap<u64, UserProfile>,
}

/// Main entry point for the example.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = "lazy_map_test.par";

    // 1. Create data
    let mut users = HashMap::new();
    for i in 0..100 {
        users.insert(
            i,
            UserProfile {
                id: i,
                name: format!("User {}", i),
                bio: "A very long bio...".repeat(100),
            },
        );
    }
    let db = UserDatabase { users };

    Parcode::save(path, &db)?;

    // 2. Read back
    let reader = ParcodeReader::open(path)?;
    let root_lazy = reader.read_lazy::<UserDatabase>()?;

    // 3. Access lazy map
    let users_lazy = root_lazy.users; // ParcodeMapPromise

    // 4. Test get_lazy(50)
    println!("Testing get_lazy(50)...");
    let user_lazy_opt = users_lazy.get_lazy(&50)?;

    assert!(user_lazy_opt.is_some(), "User 50 should exist");
    let user_lazy = user_lazy_opt.expect("User 50 should exist");

    // Verify local field access (should be instant)
    println!("User Name: {}", user_lazy.name);
    assert_eq!(user_lazy.name, "User 50");

    // Verify remote field is a promise
    println!("Loading bio...");
    let bio = user_lazy.bio.load()?;
    assert!(bio.starts_with("A very long bio..."));

    println!("Success!");

    std::fs::remove_file(path)?;
    Ok(())
}
