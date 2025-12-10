//! Comparison of Parcode, Bincode and Sled measuring access
//! Run: cargo run --example `lazy_loading_comparison`

#![allow(missing_docs)]

use parcode::{Parcode, ParcodeObject, ParcodeReader};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Write};
use std::time::Instant;
use tempfile::NamedTempFile;

#[derive(Serialize, Deserialize, Clone, ParcodeObject)]
struct BigAsset {
    id: u32,
    #[parcode(chunkable)]
    data: Vec<u8>,
}

#[derive(Serialize, Deserialize, Clone, ParcodeObject)]
struct GameWorld {
    world_name: String,
    #[parcode(chunkable)]
    skybox: BigAsset,
    #[parcode(chunkable)]
    terrain: BigAsset,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let asset_size = 50 * 1024 * 1024; // 50MB
    println!(
        "Generating world with two {} MB assets...\n",
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

    // ---------------- PARCODE ----------------
    println!("=== Parcode ===");
    let file_parcode = NamedTempFile::new().expect("temp file");
    let start_total = Instant::now();

    let start_write = Instant::now();
    Parcode::save(file_parcode.path(), &world)?;
    println!("Writting: {:.2?}", start_write.elapsed());

    let start_meta = Instant::now();
    let reader = ParcodeReader::open(file_parcode.path())?;
    let lazy_world = reader.read_lazy::<GameWorld>()?;
    println!("Metadata reading: {:.2?}", start_meta.elapsed());
    println!("World Name: {}", lazy_world.world_name);
    println!("Skybox ID: {}", lazy_world.skybox.id);
    println!("Terrain ID: {}", lazy_world.terrain.id);

    let start_sky = Instant::now();
    let sky_data = lazy_world.skybox.data.load()?;
    println!(
        "Skybox access: {:.2?} ({} bytes)",
        start_sky.elapsed(),
        sky_data.len()
    );

    println!("Parcode total time: {:.2?}", start_total.elapsed());

    // ---------------- BINCODE ----------------
    println!("\n=== Bincode ===");
    let file_bincode = NamedTempFile::new().expect("temp file");
    let start_total = Instant::now();

    let start_write = Instant::now();
    let encoded = bincode::serde::encode_to_vec(&world, bincode::config::standard())?;
    File::create(file_bincode.path())?.write_all(&encoded)?;
    println!("Writting: {:.2?}", start_write.elapsed());

    let mut buf = Vec::new();
    let start_meta = Instant::now();
    File::open(file_bincode.path())?.read_to_end(&mut buf)?;
    let decoded: GameWorld =
        bincode::serde::decode_from_slice(&buf, bincode::config::standard())?.0;
    println!(
        "Metadata reading (all loaded): {:.2?}",
        start_meta.elapsed()
    );
    println!("World Name: {}", decoded.world_name);
    println!("Skybox ID: {}", decoded.skybox.id);
    println!("Terrain ID: {}", decoded.terrain.id);

    let start_sky = Instant::now();
    let sky_data = &decoded.skybox.data;
    println!(
        "Skybox access: {:.2?} ({} bytes)",
        start_sky.elapsed(),
        sky_data.len()
    );

    println!("Bincode total time: {:.2?}", start_total.elapsed());

    /*// ---------------- SLED ----------------
    println!("\n=== Sled ===");
    let temp_dir = tempdir()?;
    let db = sled::open(temp_dir.path()).expect("open sled db");
    let start_total = Instant::now();

    let start_write = Instant::now();
    db.insert(
        "world",
        bincode::serde::encode_to_vec(&world, bincode::config::standard())?,
    )
    .unwrap();
    db.insert(
        "asset:skybox",
        bincode::serde::encode_to_vec(&world.skybox, bincode::config::standard())?,
    )
    .unwrap();
    db.insert(
        "asset:terrain",
        bincode::serde::encode_to_vec(&world.terrain, bincode::config::standard())?,
    )
    .unwrap();
    db.flush().unwrap();
    println!("Writting: {:.2?}", start_write.elapsed());

    let start_meta = Instant::now();
    let world_data: GameWorld = bincode::serde::decode_from_slice(
        &db.get("world").unwrap().unwrap(),
        bincode::config::standard(),
    )
    .unwrap()
    .0;
    println!("Metadata reading: {:.2?}", start_meta.elapsed());
    println!("World Name: {}", world_data.world_name);
    println!("Skybox ID: {}", world_data.skybox.id);
    println!("Terrain ID: {}", world_data.terrain.id);

    let start_sky = Instant::now();
    let skybox_data: BigAsset = bincode::serde::decode_from_slice(
        &db.get("asset:skybox").unwrap().unwrap(),
        bincode::config::standard(),
    )
    .unwrap()
    .0;
    println!(
        "Skybox access: {:.2?} ({} bytes)",
        start_sky.elapsed(),
        skybox_data.data.len()
    );

    println!("Sled total time: {:.2?}", start_total.elapsed());
    */

    println!("\n--- Comparison finished ---");
    Ok(())
}
