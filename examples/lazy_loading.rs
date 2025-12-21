//! Demostrates lazy and granular access to complex structures.
//! Run: cargo run --example `lazy_loading`

use parcode::{Parcode, ParcodeObject};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tempfile::NamedTempFile;

/// Asset with chunkable data.
#[derive(Serialize, Deserialize, Clone, ParcodeObject)]
struct BigAsset {
    /// Asset ID.
    id: u32,
    /// Raw data, stored in a separate chunk.
    #[parcode(chunkable)]
    data: Vec<u8>,
}

/// The game world containing assets.
#[derive(Serialize, Deserialize, Clone, ParcodeObject)]
struct GameWorld {
    /// Name of the world.
    world_name: String,

    /// Skybox asset, stored as a separate chunk.
    #[parcode(chunkable)]
    skybox: BigAsset,

    /// Terrain asset, stored as a separate chunk.
    #[parcode(chunkable)]
    terrain: BigAsset,
}

/// Main entry point for the example.
fn main() -> parcode::Result<()> {
    println!("--- Parcode Lazy Loading Example ---");
    // 1. Generate Data
    let asset_size = 50 * 1024 * 1024; // 50MB
    println!(
        "Generating world with two {} MB assets...",
        asset_size / 1024 / 1024
    );

    let world = GameWorld {
        world_name: "Azeroth".into(),
        skybox: BigAsset {
            id: 1,
            data: vec![1u8; asset_size],
        },
        terrain: BigAsset {
            id: 2,
            data: vec![2u8; asset_size],
        },
    };

    let file = NamedTempFile::new().expect("Failed to create temp file");

    println!("Saving world with Parcode...");
    let start = Instant::now();
    Parcode::save(file.path(), &world)?;
    println!("Saved in {:.2?}", start.elapsed());

    // 2. Lazy Reading
    println!("\n--- Lazy Access ---");
    let file_handle = Parcode::open(file.path())?;

    let start_lazy = Instant::now();
    let lazy_world = file_handle.root::<GameWorld>()?;
    println!("Lazy Metadata Loaded in {:.2?}", start_lazy.elapsed());
    let printed_lazy = Instant::now();
    // Access to local metadata (Instant)
    println!("World Name: {}", lazy_world.world_name);

    // Deep navigation (Instant, only reads headers)
    println!("Skybox ID: {}", lazy_world.skybox.id);
    println!("Terrain ID: {}", lazy_world.terrain.id);
    println!("Lazy Metadata Printed in {:.2?}", printed_lazy.elapsed());

    // Selective loading
    println!("Loading ONLY Skybox data...");
    let load_start = Instant::now();
    let sky_data = lazy_world.skybox.data.load()?;
    println!(
        "Loaded Skybox ({} bytes) in {:.2?}",
        sky_data.len(),
        load_start.elapsed()
    );

    println!("Done. Notice we never loaded Terrain data!");
    println!("Time elapsed with Parcode: {:.2?}", start_lazy.elapsed());

    Ok(())
}
