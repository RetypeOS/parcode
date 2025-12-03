# Parcode

[![Crates.io](https://img.shields.io/crates/v/parcode.svg)](https://crates.io/crates/parcode)
[![Docs.rs](https://docs.rs/parcode/badge.svg)](https://docs.rs/parcode)
[![CI](https://github.com/retypeos/parcode/actions/workflows/ci.yml/badge.svg)](https://github.com/retypeos/parcode/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](./LICENSE-MIT)

---

A high-performance, parallelized library for caching complex Rust data structures to disk.

`parcode` is not just another serialization wrapper. It is an opinionated, architecture-aware caching system designed from the ground up to conquer the bottleneck between CPU-intensive serialization and I/O-bound disk access.

## Core Philosophy: Decouple Everything

Traditional serialization (`serde` + `bincode` + `File::create`) is a sequential process: you serialize a large object into a buffer in memory, and then you write that buffer to disk. For large datasets, this is inefficient.

`parcode`'s core insight is to treat serialization and disk I/O as separate stages of a concurrent pipeline:

*   **For Writing:** It uses a **Batched Producer-Consumer** architecture. A pool of `rayon` threads (producers) furiously serializes and compresses small chunks of your data in parallel. A single, dedicated I/O thread (the consumer) receives these ready-to-write byte buffers and streams them to the disk sequentially. A bounded channel between them provides automatic backpressure, preventing memory overload if the disk is slow.

*   **For Reading:** It uses a **Parallel `mmap` Pipeline**. The file is memory-mapped, providing near-zero-cost access managed by the OS. `rayon` threads then work in parallel, each grabbing a chunk's data from the `mmap`, decompressing, and deserializing it directly into its final location in a pre-allocated buffer. This maximizes I/O and CPU concurrency.

The result is a library that dramatically outperforms traditional methods for medium to large datasets.

## Key Features

*   🚀 **Blazing Fast Writes:** The concurrent pipeline leverages all available CPU cores to saturate disk write speeds.
*   ⚡️ **Extremely Fast Reads:** `mmap`-based parallel deserialization minimizes I/O overhead and maximizes CPU usage.
*   🧠 **Low-Memory Streaming:** Process datasets larger than RAM using efficient iterator-based APIs.
*   🗜️ **Pluggable Compression:** Ships with optional `lz4_flex` support via a feature flag. The architecture is extensible for other algorithms.
*   🛡️ **Robust File Format:** The custom format (`.pcode`) is versioned, uses checksums to ensure metadata integrity, and is designed for performance.

## Quick Start

This example demonstrates the most common use case: writing a large collection to a file and reading it back.

```rust
use parcode::{Parcode, Result};
use serde::{Serialize, Deserialize};
use tempfile::tempdir;

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
struct UserProfile {
    id: u32,
    username: String,
}

// Enable Parcode behavior for this struct
// This allows it to be efficiently split into chunks
use parcode::ParcodeObject;

#[derive(ParcodeObject)] // Optional: derive macro if available/needed for custom splitting
struct MyData {
    users: Vec<UserProfile>,
}

fn main() -> Result<()> {
    // 1. Prepare some data.
    let users: Vec<_> = (0..10_000)
        .map(|i| UserProfile { id: i, username: format!("user_{}", i) })
        .collect();

    let dir = tempdir()?;
    let file_path = dir.path().join("user_cache.pcode");

    // 2. Write the collection to a file.
    // `parcode` will automatically split it into chunks and write them in parallel.
    Parcode::save(&file_path, &users)?;

    // 3. Read the entire collection back.
    // `parcode` will read and deserialize the chunks in parallel.
    let restored_users: Vec<UserProfile> = Parcode::read(&file_path)?;

    // 4. Verify the data.
    assert_eq!(users.len(), restored_users.len());
    assert_eq!(users[0], restored_users[0]);

    println!("Successfully wrote and read {} user profiles.", users.len());
    Ok(())
}
```

## Performance

`parcode` is optimized for medium-to-large collections. Below are representative results from our benchmarks comparing a parallel `parcode` write/read against a standard, buffered `bincode` write/read.

> *(Results from an 8-core machine writing/reading 1 million complex objects)*

| Scenario                  | `raw bincode` | `parcode` | Speedup      |
| ------------------------- | ------------- | --------- | ------------ |
| **Write Collection**      | ~1.9 s        | ~1.3 s    | **~1.5x**    |
| **Read Full Collection**  | ~3.1 s        | ~1.1 s    | **~2.8x**    |
| **Streaming Read**        | ~2.4 s        | ~1.3 s    | **~1.8x**    |

For very small collections (< 1000 items), the overhead of `parcode`'s concurrent pipeline may be slightly slower than a direct `bincode` write, but `parcode`'s performance advantage grows dramatically with data size.

## When to Use `parcode`

`parcode` excels in scenarios where I/O performance for large, complex Rust data is critical.

**Ideal Use Cases:**

*   Caching expensive computations or database query results.
*   Passing large datasets between stages in an ETL or data processing pipeline.
*   Saving and loading application state (e.g., in scientific computing or simulations).
*   Any situation where you are writing/reading `Vec`s or `HashMap`s with thousands of elements or more.

**When NOT to Use `parcode`:**

*   For human-readable configuration files (use `serde_json` or `serde_yaml`).
*   For persisting single, very small objects where single-digit millisecond overhead is unacceptable.
*   When interoperability with non-Rust programs is required.

## Installation

Add `parcode` to your `Cargo.toml`:

```toml
[dependencies]
parcode = "0.3.1"
```

## Cargo Features

*   `lz4_flex` (disabled by default): Enables LZ4 compression support.

    ```toml
    [dependencies]
    parcode = { version = "0.3.1", features = ["lz4_flex"] }
    ```

## License

This project is licensed under either of

*   Apache License, Version 2.0, ([LICENSE-APACHE](./LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
*   MIT license ([LICENSE-MIT](./LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

## Contributing

Contributions are welcome! Please feel free to submit a pull request or open an issue.
