
# *Parcode* [OUTDATED. Probably not correspond to new project reality ]

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

* **For Writing:** It uses a **Batched Producer-Consumer** architecture. A pool of `rayon` threads (producers) furiously serializes and compresses small chunks of your data in parallel. A single, dedicated I/O thread (the consumer) receives these ready-to-write byte buffers and streams them to the disk sequentially. A bounded channel between them provides automatic backpressure, preventing memory overload if the disk is slow.

* **For Reading:** It uses a **Parallel `mmap` Pipeline**. The file is memory-mapped, providing near-zero-cost access managed by the OS. `rayon` threads then work in parallel, each grabbing a chunk's data from the `mmap`, decompressing, and deserializing it directly into its final location in a pre-allocated buffer. This maximizes I/O and CPU concurrency.

The result is a library that dramatically outperforms traditional methods for medium to large datasets.

## Key Features

* 🚀 **Blazing Fast Writes:** The concurrent pipeline leverages all available CPU cores to saturate disk write speeds.
* ⚡️ **Extremely Fast Reads:** `mmap`-based parallel deserialization minimizes I/O overhead and maximizes CPU usage.
* 🧠 **Low-Memory Streaming:** Process datasets larger than RAM using efficient iterator-based APIs (`iter_collection`) that keep memory usage constant and minimal.
* ✨ **Ergonomic Macros:** `read_chunks!` and `write_chunks!` provide a clean, declarative syntax for handling heterogeneous data.
* 🗜️ **Pluggable Compression:** Ships with optional `lz4_flex` support via a feature flag. The architecture is extensible for other algorithms.
* 🛡️ **Robust File Format:** The custom format (`.pcode`) is versioned, uses checksums to ensure metadata integrity, and is designed for performance.

## Quick Start

This example demonstrates the most common use case: writing a large collection to a file and reading it back.

```rust
use parcode::{ParcodeWriter, ParcodeReader, SplitStrategy, Result};
use serde::{Serialize, Deserialize};
use tempfile::tempdir;

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
struct UserProfile {
    id: u32,
    username: String,
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
    let writer = ParcodeWriter::new();
    writer.write_collection(&file_path, 0, &users, SplitStrategy::Automatic)?;

    // 3. Read the entire collection back.
    // `parcode` will read and deserialize the chunks in parallel.
    let reader = ParcodeReader::new(&file_path)?;
    let restored_users: Vec<UserProfile> = reader.read_and_reassemble_collection(0)?;

    // 4. Verify the data.
    assert_eq!(users.len(), restored_users.len());
    assert_eq!(users[0], restored_users[0]);

    println!("Successfully wrote and read {} user profiles.", users.len());
    Ok(())
}
```

## Advanced Usage

### Streaming Reads for Low Memory

When your dataset is too large to fit in memory, use `iter_collection` to process it chunk by chunk.

```rust
# use parcode::{ParcodeReader, Result, ParcodeWriter, SplitStrategy};
# use serde::{Serialize, Deserialize};
# #[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
# struct UserProfile { id: u32, username: String }
# fn main() -> Result<()> {
# let users: Vec<_> = (0..100).map(|i| UserProfile { id: i, username: format!("user_{}", i) }).collect();
# let dir = tempfile::tempdir()?;
# let file_path = dir.path().join("user_cache.pcode");
# ParcodeWriter::new().write_collection(&file_path, 0, &users, SplitStrategy::Automatic)?;
let reader = ParcodeReader::new(&file_path)?;

// This iterator processes one chunk at a time, keeping peak memory usage low.
for chunk_result in reader.iter_collection::<UserProfile>(0)? {
    let user_chunk = chunk_result?;
    println!("Processing a chunk of {} users...", user_chunk.len());
    // ... do work with `user_chunk` ...
}
# Ok(())
# }
```

### Heterogeneous Read & Write with Macros

Use the `write_chunks!` and `read_chunks!` macros for maximum ergonomics when dealing with different data types in the same file.

```rust
# use parcode::{ParcodeWriter, ParcodeReader, write_chunks, read_chunks, Result};
# use serde::{Serialize, Deserialize};
# #[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
# struct UserProfile { id: u32, name: String }
# #[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
# struct AppConfig { theme: String }
# fn main() -> Result<()> {
let writer = ParcodeWriter::new();
let user = UserProfile { id: 1, name: "Alice".to_string() };
let config = AppConfig { theme: "dark".to_string() };
let log_entries = vec!["entry 1".to_string(), "entry 2".to_string()];
let file_path = tempfile::tempdir()?.path().join("hetero.pcode");

// Write different types with clear, declarative syntax.
write_chunks!(&writer, &file_path, {
    object 0 => &user;
    object 1 => &config;
    collection 100 => &log_entries;
})?;

let reader = ParcodeReader::new(&file_path)?;

// Read them back into typed variables in a single parallel operation.
let (read_user, read_config, read_logs): (UserProfile, AppConfig, Vec<String>) = read_chunks!(&reader, {
    0 => UserProfile,
    1 => AppConfig,
    100 => Vec<String>,
})?;

assert_eq!(user, read_user);
assert_eq!(config, read_config);
assert_eq!(log_entries, read_logs);
# Ok(())
# }
```

### Writing from an Iterator

For convenience, you can write directly from any iterator without collecting into a `Vec` first. `parcode` will consume the iterator and write it in chunks using a low-memory streaming pipeline.

```rust
# use parcode::{ParcodeWriter, SplitStrategy, Result};
# fn main() -> Result<()> {
# let writer = ParcodeWriter::new();
# let path = tempfile::tempdir()?.path().join("iter.pcode");
let my_iterator = (0..100_000).filter(|x| x % 3 == 0);

// `write_collection_from_iter` requires specifying the number of items per chunk.
writer.write_collection_from_iter(path, 0, my_iterator, 8192)?;
# Ok(())
# }
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

* Caching expensive computations or database query results.
* Passing large datasets between stages in an ETL or data processing pipeline.
* Saving and loading application state (e.g., in scientific computing or simulations).
* Any situation where you are writing/reading `Vec`s or `HashMap`s with thousands of elements or more.

**When NOT to Use `parcode`:**

* For human-readable configuration files (use `serde_json` or `serde_yaml`).
* For persisting single, very small objects where single-digit millisecond overhead is unacceptable.
* When interoperability with non-Rust programs is required.

## Installation

Add `parcode` to your `Cargo.toml`:

```toml
[dependencies]
parcode = "0.1.0"
```

## Cargo Features

* `lz4_flex` (disabled by default): Enables LZ4 compression support.

    ```toml
    [dependencies]
    parcode = { version = "0.1.0", features = ["lz4_flex"] }
    ```

## License

This project is licensed under either of

* Apache License, Version 2.0, ([LICENSE-APACHE](./LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
* MIT license ([LICENSE-MIT](./LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

## Contributing

Contributions are welcome! Please feel free to submit a pull request or open an issue.
