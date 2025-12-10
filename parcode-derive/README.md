# Parcode Derive

[![Crates.io](https://img.shields.io/crates/v/parcode-derive.svg)](https://crates.io/crates/parcode-derive)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

This crate provides the `#[derive(ParcodeObject)]` procedural macro for the [Parcode](https://crates.io/crates/parcode) library.

> **Note:** You likely do not need to add this crate directly to your `Cargo.toml`. Instead, use the `parcode` crate which re-exports this macro.

## Usage

Deriving `ParcodeObject` on a struct automatically implements:

1. **Serialization Logic:** Optimized, parallel-ready serialization.
2. **Lazy Mirroring:** Generates a shadow struct (e.g., `MyStructLazy`) for zero-copy field access.

```rust
use parcode::ParcodeObject;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, ParcodeObject)]
struct Level {
    // Local fields (stored in the main payload)
    id: u32,
    name: String,

    // Chunkable fields (stored in separate shards for parallel access)
    #[parcode(chunkable)]
    geometry: Vec<u8>,

    // Optimized Map (O(1) random access via hashing)
    #[parcode(map)]
    users: std::collections::HashMap<u64, String>,
    
    // Custom compression per field
    #[parcode(chunkable, compression = "lz4")]
    assets: Vec<u8>,
}
```

## Attributes

- **`#[parcode(chunkable)]`**: Marks a field (usually a `Vec` or nested Struct) to be stored in its own graph node. This enables lazy loading and parallel writing for this field.
- **`#[parcode(map)]`**: Activates "Hash Sharding" for `HashMap` types. This splits the map into buckets and builds micro-indexes, allowing $O(1)$ lookup without deserializing the whole map.
- **`#[parcode(compression = "...")]`**: Overrides the compression algorithm for this specific field. Supported values: `"none"`, `"lz4"`.

## License

This project is licensed under the [MIT license](LICENSE).
