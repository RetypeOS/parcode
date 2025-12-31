# Parcode

[![Crates.io](https://img.shields.io/crates/v/parcode.svg)](https://crates.io/crates/parcode)
[![Docs.rs](https://docs.rs/parcode/badge.svg)](https://docs.rs/parcode)
[![CI](https://github.com/retypeos/parcode/actions/workflows/ci.yml/badge.svg)](https://github.com/retypeos/parcode/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

---

**High-performance, zero-copy, lazy-loading object storage for Rust.**

Parcode is a Rust persistence library designed for **true lazy access**.

It lets you open massive object graphs and access a single field, record,
or asset without deserializing the rest of the file.

This enables capabilities previously reserved for complex databases:

* **Lazy Mirrors:** Navigate deep struct hierarchies without loading data from disk.
* **Surgical Access:** Load only the specific field, vector chunk, or map entry you need.
* **$O(1)$ Map Lookups:** Retrieve items from huge `HashMap`s instantly without full deserialization.
* **Parallel Speed:** Writes are fully parallelized using a Zero-Copy graph architecture.

---

## The Innovation: Pure Rust Lazy Loading

Most libraries that offer "Lazy Loading" or "Zero-Copy" access (like FlatBuffers or Cap'n Proto) come with a heavy price: **Interface Definition Languages (IDLs)**. You are forced to write separate schema files (`.proto`, `.fbs`), run external compilers, and deal with generated code that doesn't feel like Rust.

**Parcode changes the game.**

We invented a technique we call **"Compile-Time Structural Mirroring (CTSM)"**. By simply adding `#[derive(ParcodeObject)]`, Parcode analyzes your Rust structs at compile time and invisibly generates a **Lazy Mirror** API.

|          Feature         |    FlatBuffers / Cap'n Proto    |        **Parcode**         |
| :----------------------- | :------------------------------ | :------------------------- |
| **Schema Definition**    | External IDL files (`.fbs`)     | **Standard Rust Structs**  |
| **Build Process**        | Requires external CLI (`flatc`) | **Standard `cargo build`** |
| **Refactoring**          | Manual sync across files        | **IDE Rename / Refactor**  |
| **Developer Experience** | Foreign                         | **Native**                 |

More info in the [whitpaper](whitepaper.md).

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
parcode = "0.6"
```

To enable LZ4 compression:

```toml
[dependencies]
parcode = { version = "0.6", features = ["lz4_flex"] }
```

### Features

* `mmap`: Enable memory mapping
* `parallel`: Enable parallelization
* `standard`: Default feature, activate features 'mmap' and 'parallel' (used as Default)
* `lz4_flex`: Enable LZ4 compression to ID 1.

---

## Usage Guide

### 1. Define your Data

Use `#[derive(ParcodeObject)]` and the `#[parcode(...)]` attributes to tell the engine how to shard your data.

```rust
use parcode::ParcodeObject; // Use ParcodeObject trait to enable lazy procedural macros
use serde::{Serialize, Deserialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, ParcodeObject)]
struct GameWorld {
    id: u64,               // Stored Inline (Metadata)
    name: String,          // Stored Inline (Metadata)

    #[parcode(chunkable)]  // Stored in a separate chunk
    settings: WorldSettings,

    #[parcode(chunkable, compression = 'lz4_flex')]  // Automatically sharded into parallel chunks and compress it.
    terrain: Vec<u8>, 
    
    #[parcode(map)]        // Hash-sharded for O(1) lookups
    players: HashMap<String, Player>,
}

#[derive(Serialize, Deserialize, ParcodeObject, Clone)]
struct WorldSettings {
    difficulty: u8,
    #[parcode(chunkable)]
    history: Vec<String>,
}

#[derive(Serialize, Deserialize, ParcodeObject, Clone)]
struct Player {
    level: u32,
    #[parcode(chunkable)]
    inventory: Vec<u32>, // Heavy data
}
```

### 2. Save Data

You have two ways to save data: Simple and Configured.

**A. Simple Save (Default Settings)**
Perfect for quick prototyping.

```rust
use parcode::Parcode;

let world = GameWorld { /* ... */ };

// Saves with parallelism enabled
Parcode::save("savegame.par", &world)?;
```

**B. Configured Save**
Use the builder mode.

```rust
// Saves with LZ4 compression on ALL and write parcode serialized data into a new file.
Parcode::builder()
    .compression(true)
    .save("savegame_compressed.par", &world)?;
```

### 3. Read Data (Lazy)

Here is where the magic happens. We don't load the object; we load a **Mirror**.

```rust
use parcode::Parcode;

// 1. Open the file (Instant, uses mmap)
let reader = Parcode::open("savegame.par")?;

// 2. Get the Lazy Mirror (Instant, reads only header)
// Note: We get 'GameWorldLazy', a generated shadow struct.
let world_mirror = reader.root::<GameWorld>()?; // Or .load_lazy(), is the same.

// 3. Access local fields directly (Already in memory)
println!("World ID: {}", world_mirror.id);

// 4. Navigate hierarchy without I/O
// 'settings' is a mirror. Accessing it costs nothing.
// 'difficulty' is inline. Accessing it costs nothing.
println!("Difficulty: {}", world_mirror.settings.difficulty);

// 5. Surgical Load
// Only NOW do we touch disk to load the history vector.
// The massive 'terrain' vector is NEVER loaded.
let history = world_mirror.settings.history.load()?;
```

### 4. Advanced Access Patterns

#### O(1) Map Lookup

Retrieve a single user from a million-user database without loading the database.

```rust
// .get() returns a full object
// .get_lazy() returns a Mirror of the object!

if let Some(player_mirror) = world_mirror.players.get_lazy(&"Hero123".to_string())? {
    // Access player metadata instantly
    println!("Player Level: {}", player_mirror.level);
    
    // Only load inventory if needed
    let inv = player_mirror.inventory.load()?;
}
```

#### Lazy Vector Iteration

Scan a list of heavy objects without loading their heavy payloads.

```rust
// Assume we have Vec<Player>
for Ok(player_proxy) in world_mirror.all_players.iter_lazy()? {
    
    // We can check level WITHOUT loading the player's inventory of all players from disk, only you needed!
    if p.level > 50 {
        println!("High level player found!");
        p.inventory.load()?; 
    }
}
```

---

## Advanced Features

### Generic I/O: Write to Memory/Network

Parcode isn't limited to files. You can serialize directly to any `std::io::Write` destination.

```rust
let mut buffer = Vec::new();

// Serialize directly to RAM
Parcode::write(&mut buffer, &my_data)?;

// 'buffer' now contains the full Parcode serialized structure
```

### Synchronous Mode

For environments where threading is not available (WASM, embedded) or to reduce memory overhead.

```rust
Parcode::write_sync("sync_save.par", &data)?;
```

### Inspector

Parcode includes tools to analyze the structure of your files without deserializing them.

```rust
use parcode::Parcode;

let report = Parcode::inspect("savegame.par")?;
println!("{}", report);
```

**Output:**

```text
=== PARCODE INSPECTOR REPORT ===
Root Offset:    550368
[GRAPH LAYOUT]
└── [Generic Container] Size: 1b | Algo: None | Children: 2
    ├── [Vec Container] Size: 13b | Algo: LZ4 | Children: 32 [Vec<50000> items] 
    └── [Map Container] Size: 4b | Algo: None | Children: 4 [Hashtable with 4 buckets]
```

**Note**: `Algo` means compression algorithm.

---

## Macro Attributes Reference

Control exactly how your data structure maps to disk using `#[parcode(...)]`.

|             Attribute           |                     Effect                     |            Best For                                     |
| :------------------------------ | :--------------------------------------------- | :------------------------------------------------------ |
| **(none)**                      | Field is serialized into the parent's payload. | Small primitives (`u32`, `bool`), short Strings, flags. |
| `#[parcode(chunkable)]`         | Field is stored in its own independent Chunk.  | Structs, Vectors, or fields you want to load lazily (`.load()`).   |
| `#[parcode(map)]`               | Field (`HashMap`) is sharded by hash.          | Large Dictionaries/Indices where you need random access (`.get()`). |
| `#[parcode(compression="lz4")]` | Overrides compression for this chunk.          | Highly compressible data (text, save states).      |

---

## Benchmarks

> **Scenario:** Opening a persisted world state (~10 MB) and accessing a single piece of data.

| Serializer  |   Cold Start | Deep Field Access |    Map Lookup |     Total (Cold + Targeted Access) |
| ----------- | -----------: | ----------------: | ------------: | ---------------------------------: |
| **Parcode** | **~1.38 ms** |      ~0.000017 ms |   ~0.00016 ms | **~1.38 ms + ~0.0001 ms / target** |
| Cap’n Proto |       ~60 ms |      ~0.000046 ms |   ~0.00437 ms |        ~60 ms + ~0.004 ms / target |
| Postcard    |       ~80 ms |      ~0.000017 ms |  ~0.000017 ms |      ~80 ms + ~0.00002 ms / target |
| Bincode     |      ~299 ms |      ~0.000015 ms | ~0.0000025 ms |     ~299 ms + ~0.00001 ms / target |

---

### This table shows

* **Cold Start** dominates total latency for traditional serializers
* **Targeted access cost is negligible** *once data is loaded*
* Therefore, **real-world point access latency ≈ cold-start time**

Parcode is the only system where:

* Cold start is *constant*
* Access cost scales only with the data actually requested
* Unused data is never deserialized

---

### Key takeaway

> **True laziness is not about fast reads — it is about avoiding unnecessary work.**

Parcode minimizes **observable latency** by paying only for:

`Cold start (structural metadata) + exactly the data you touch (and his shard)`

*Benchmarks run on NVMe SSD. Parallel throughput scales with cores.*

---

## License

This project is licensed under the [MIT license](LICENSE).

*Built for the Rust community by RetypeOS.*
