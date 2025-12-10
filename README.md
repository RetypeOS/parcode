# Parcode

[![Crates.io](https://img.shields.io/crates/v/parcode.svg)](https://crates.io/crates/parcode)
[![Docs.rs](https://docs.rs/parcode/badge.svg)](https://docs.rs/parcode)
[![CI](https://github.com/retypeos/parcode/actions/workflows/ci.yml/badge.svg)](https://github.com/retypeos/parcode/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

---

**High-performance, zero-copy, lazy-loading object storage for Rust.**

`parcode` is an architecture-aware storage system designed for complex, deep data structures. Unlike traditional serialization (JSON, Bincode) which treats data as a flat blob, `parcode` preserves the **structure** of your objects on disk.

This enables capabilities previously reserved for complex databases:

* **Lazy Mirrors:** Navigate deep struct hierarchies without loading data from disk.
* **Surgical Access:** Load only the specific field, vector chunk, or map entry you need.
* **$O(1)$ Map Lookups:** Retrieve items from huge `HashMap`s instantly without full deserialization.
* **Parallel Speed:** Writes are fully parallelized using a Zero-Copy graph architecture.

---

## The Innovation: Pure Rust Lazy Loading

Most libraries that offer "Lazy Loading" or "Zero-Copy" access (like FlatBuffers or Cap'n Proto) come with a heavy price: **Interface Definition Languages (IDLs)**. You are forced to write separate schema files (`.proto`, `.fbs`), run external compilers, and deal with generated code that doesn't feel like Rust.

**Parcode changes the game.**

We invented a technique we call **"Native Mirroring"**. By simply adding `#[derive(ParcodeObject)]`, Parcode analyzes your Rust structs at compile time and invisibly generates a **Lazy Mirror** API.

| Feature | FlatBuffers / Cap'n Proto | **Parcode** |
| :--- | :--- | :--- |
| **Schema Definition** | External IDL files (`.fbs`) | **Standard Rust Structs** |
| **Build Process** | Requires external CLI (`flatc`) | **Standard `cargo build`** |
| **Refactoring** | Manual sync across files | **IDE Rename / Refactor** |
| **Developer Experience** | Foreign | **Native** |

### How it works

You define your data naturally:

```rust
#[derive(ParcodeObject)]
struct Level {
    name: String,
    #[parcode(chunkable)]
    physics: PhysicsData, // Heavy struct
}
```

Parcode's macro engine automatically generates a **Shadow Type** (`LevelLazy`) that mirrors your structure but replaces heavy fields with **Smart Promises**.

* When you call `reader.read_lazy::<Level>()`, you don't get a `Level`.
* You get a `LevelLazy` handle.
* Accessing `level_lazy.name` is instant (read from header).
* Accessing `level_lazy.physics` returns a **Promise**, not data.
* Only calling `.load()` triggers the disk I/O.

**Result:** The performance of a database with the ergonomics of a standard `struct`.

---

## The Principal Feature: Lazy Mirrors

The problem with standard serialization is **"All or Nothing"**. To read `level.config.name`, you usually have to deserialize the entire 500MB `level` file.

**Parcode V3 solves this.** By simply adding `#[derive(ParcodeObject)]`, the library generates a "Shadow Mirror" of your struct. You can traverse this mirror instantly—only metadata is read—and trigger disk I/O only when you actually request the data.

### Example

```rust
use parcode::{Parcode, ParcodeObject, ParcodeReader};
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, ParcodeObject)]
struct Level {
    id: u32,               // Local field (Metadata)
    name: String,          // Local field (Metadata)

    #[parcode(chunkable)]  // Stored in a separate, compressed chunk
    config: LevelConfig,

    #[parcode(chunkable)]  // Large collection (sharded automatically)
    assets: Vec<u8>, 
}

#[derive(Serialize, Deserialize, ParcodeObject)]
struct LevelConfig {
    version: u8,
    #[parcode(chunkable)]
    metadata: String,      // Deeply nested chunk
}

fn main() -> parcode::Result<()> {
    // 1. Open the file (Instant operation, mmaps the content)
    let reader = ParcodeReader::open("level.par")?;

    // 2. Get the Lazy Mirror (Instant operation, reads only the header)
    let level_lazy = reader.read_lazy::<Level>()?;

    // 3. Access local fields directly (Already in memory)
    println!("ID: {}, Name: {}", level_lazy.id, level_lazy.name);

    // 4. Navigate deep without loading!
    // 'config' is a Mirror. Accessing it costs 0 I/O.
    // 'version' is a local field of config. Accessing it costs 0 I/O (eager header load).
    println!("Config Version: {}", level_lazy.config.version);

    // 5. Surgical Load
    // Only NOW do we touch the disk to load the specific 'metadata' chunk.
    // The 2GB 'assets' vector is NEVER loaded.
    let meta = level_lazy.config.metadata.load()?;
    println!("Deep Metadata: {}", meta);

    Ok(())
}
```

This architecture allows "Cold Starts" in **microseconds**, regardless of the file size.

---

## O(1) Map Access: The "Database" Mode

Storing a `HashMap` usually means serializing it as a single huge blob. If you need one user from a 1GB user database, you have to read 1GB.

Parcode introduces **Hash Sharding**. By marking a map with `#[parcode(map)]`, the library automatically:

1. Distributes items into buckets based on their hash.
2. Writes each bucket as an independent chunk with a "Micro-Index" (Structure of Arrays).
3. Allows **O(1) retrieval** by reading only the relevant ~4KB shard.

```rust
#[derive(Serialize, Deserialize, ParcodeObject)]
struct UserDatabase {
    // Standard Mode: Good for small maps or full scans.
    #[parcode(chunkable)] 
    settings: HashMap<String, String>,

    // Database Mode: Essential for large datasets (O(1) lookup).
    #[parcode(map)]
    users: HashMap<u64, UserProfile>, 
}

// Usage
let db = reader.read_lazy::<UserDatabase>()?;

// Retrieves ONE user. 
// Cost: ~40µs. RAM Usage: ~50KB.
// The other 999,999 users are never touched.
let user = db.users.get(&88888)?.expect("User not found");
```

---

## Macro Attributes Reference

Control exactly how your data structure maps to disk using `#[parcode(...)]`.

| Attribute | Effect | Best For |
| :--- | :--- | :--- |
| **(none)** | Field is serialized into the parent's payload. | Small primitives (`u32`, `bool`), short Strings, flags. Access is instant if parent is loaded. |
| `#[parcode(chunkable)]` | Field is stored in its own independent Chunk (node). | Large structs, vectors, or fields you want to load lazily (`.load()`). |
| `#[parcode(map)]` | Field (`HashMap`) is sharded by hash. | Large Dictionaries/Indices where you need random access by key (`.get()`). |
| `#[parcode(compression="lz4")]` | Overrides compression for this specific field/chunk. | Highly compressible data (text, save states). Requires `lz4_flex` feature. |

### Smart Defaults

Parcode is adaptive.

* **Small Vectors:** If a `Vec` marked `chunkable` is tiny (< 4KB), Parcode may inline it or merge chunks to avoid overhead.
* **Small Maps:** If a `HashMap` marked `map` has few items (< 200), Parcode automatically falls back to standard serialization to save space.

---

## Benchmarks vs The World

We benchmarked Parcode V3 against `bincode` (the Rust standard for raw speed) and `sled` (an embedded DB) in a complex game world scenario involving heavy assets (100MB) and metadata lookups.

> **Scenario:** Cold Start of an application reading a massive World State file.

| Operation | Tool | Time | Memory (Peak) | Notes |
| :--- | :--- | :--- | :--- | :--- |
| **Cold Start** (Ready to read metadata) | **Parcode** | **0.16 ms** | **0 MB** | Instant. Only headers read. |
| | Bincode | 97.47 ms | 30 MB | Forced to deserialize everything. |
| **Deep Fetch** (Load 1 asset) | **Parcode** | **3.20 ms** | **3.8 MB** | Loads only the target 1MB chunk. |
| | Bincode | 97.47 ms | 30 MB | Same cost as full load. |
| **Map Lookup** (Find user by ID) | **Parcode** | **0.02 ms** | **0 MB** | **4000x Faster**. Hash Sharding win. |
| | Bincode | 97.47 ms | 30 MB | O(N) scan. |
| **Write Speed** (Throughput) | **Parcode** | ~73 ms | 48 MB | Slightly slower due to graph building (Depending of dataset data). |
| | Bincode | ~50 ms | 0.01 MB | Faster but produces monolithic blobs. |

*Benchmarks run on NVMe SSD. Parallel throughput scales with cores.*

---

## Real-World Scenario: Heavy Game Assets

Benchmarks are useful, but how does it feel in a real application?
We simulated a Game Engine loading a World State containing two **50MB binary assets** (Skybox and Terrain) plus metadata.

> **Test:** Write to disk, restart application, read metadata (World Name), and load *only* the Skybox.
> *Note: Compression disabled to measure pure architectural efficiency.*

| Operation | Parcode | Bincode | Comparison |
| :--- | :--- | :--- | :--- |
| **Save World** (Write) | **3.37 s** | 8.88 s | **2.6x Faster** (Parallel Writes) |
| **Start Up** (Read Metadata) | **358 µs** | 10.84 s | **~30,000x Faster** (Lazy vs Full) |
| **Load Skybox** (Partial) | 1.33 s | N/A | Granular loading |
| **Total Workflow** | **4.70 s** | 19.72 s | **4.2x Faster** |

**The User Experience Difference:**

* **With Bincode:** The user stares at a frozen loading screen for **10.8 seconds** just to see the level name. Memory usage spikes to load assets that might not even be visible yet.
* **With Parcode:** The application opens **instantly (<1ms)**. The UI populates immediately. The 50MB Skybox streams in smoothly over 1.3 seconds. The Terrain is never loaded if the user doesn't look at it.

---

## Architecture Under the Hood

Parcode treats your data as a **Dependency Graph**, not a byte stream.

1. **Zero-Copy Write:** The serializer "borrows" your data (`&[T]`) instead of cloning it. It builds a graph of `ChunkNode`s representing your struct hierarchy.
2. **Parallel Execution:** Writing is orchestrated by a graph executor. Independent nodes (chunks) are serialized, compressed, and written to disk concurrently.
3. **Physical Layout:** The file format (`.pcode` v4) places children before parents ("Bottom-Up"). This allows the Root Node at the end of the file to contain a table of contents for the entire structure.
4. **Lazy Mirroring:** The `ParcodeObject` macro generates a "Mirror Struct" that holds `ChunkNode` handles instead of data. Accessing `mirror.field` simply returns another Mirror or a Promise, incurring zero I/O cost until the final `.load()`.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
parcode = "0.3.1"
```

To enable LZ4 compression:

```toml
[dependencies]
parcode = { version = "0.3.1", features = ["lz4_flex"] }
```

## License

This project is licensed under the [MIT license](LICENSE).

---
*Built for the Rust community by RetypeOS.*
