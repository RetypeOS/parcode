// --- examples/benchmark_sync.rs ---

//! Benchmark comparison: Parallel (Rayon) vs Synchronous Execution.
//!
//! Usage:
//! cargo run --release --example `benchmark_sync`

#![allow(missing_docs)]
use parcode::{Parcode, ParcodeObject};
use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Serialize, Deserialize, ParcodeObject, Clone)]
struct LargeWorld {
    id: u64,
    #[parcode(chunkable)]
    entities: Vec<Entity>,
    #[parcode(chunkable)]
    terrain: Vec<u8>,
}

#[derive(Serialize, Deserialize, ParcodeObject, Debug, Clone)]
struct Entity {
    x: f32,
    y: f32,
    z: f32,
    name: String,
    // Simulate some payload
    data: Vec<u64>,
}

fn main() -> parcode::Result<()> {
    println!("--- Parcode Sync vs Async Benchmark ---");

    // 1. Generate heavy data
    println!("Generating data...");
    let data = generate_data(500_000); // Adjust size for your machine

    // Warmup filesystem (optional)
    let _ = std::fs::remove_file("bench_async.par");
    let _ = std::fs::remove_file("bench_sync.par");

    // --- ASYNC TEST ---
    print_ram("Start Async");
    let start_async = Instant::now();

    Parcode::save("bench_async.par", &data)?;

    let duration_async = start_async.elapsed();
    println!("Async Save Time: {:.2?}", duration_async);
    print_ram("End Async");

    // --- SYNC TEST ---
    print_ram("Start Sync");
    let start_sync = Instant::now();

    Parcode::save_sync("bench_sync.par", &data)?;

    let duration_sync = start_sync.elapsed();
    println!("Sync Save Time:  {:.2?}", duration_sync);
    print_ram("End Sync");

    // --- COMPARISON ---
    println!("\n--- Results ---");
    let diff = if duration_async < duration_sync {
        let x = duration_sync.as_secs_f64() / duration_async.as_secs_f64();
        format!("Async is {:.2}x faster", x)
    } else {
        let x = duration_async.as_secs_f64() / duration_sync.as_secs_f64();
        format!("Sync is {:.2}x faster (Unexpected for large data)", x)
    };
    println!("{}", diff);

    // Clean up
    std::fs::remove_file("bench_async.par")?;
    std::fs::remove_file("bench_sync.par")?;

    Ok(())
}

fn generate_data(count: usize) -> LargeWorld {
    let entities: Vec<Entity> = (0..count)
        .map(|i| {
            Entity {
                x: i as f32,
                y: (i * 2) as f32,
                z: (i % 100) as f32,
                name: format!("Entity_{}", i),
                data: vec![i as u64; 20], // Small payload per entity
            }
        })
        .collect();

    let terrain = vec![0u8; 1024 * 1024 * 50]; // 50MB Blob

    LargeWorld {
        id: 1,
        entities,
        terrain,
    }
}

fn print_ram(_label: &str) {
    // TODO: Add memory stats.
}
