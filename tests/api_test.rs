#![allow(missing_docs)]

use parcode::{Parcode, ParcodeObject};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, ParcodeObject, PartialEq, Debug, Clone)]
struct SimpleData {
    id: u32,
    message: String,
}

#[derive(Serialize, Deserialize, ParcodeObject, PartialEq, Debug, Clone)]
struct ComplexData {
    title: String,
    #[parcode(chunkable)]
    numbers: Vec<u64>,
    #[parcode(chunkable)]
    inner: SimpleData,
}

// Generator of data
fn create_complex_data() -> ComplexData {
    ComplexData {
        title: "Integration Test".to_string(),
        numbers: (0..50_000).collect(),
        inner: SimpleData {
            id: 42,
            message: "Hello World".to_string(),
        },
    }
}

// --- TESTS ---

/// Standard File IO
/// Validate `Parcode::save`, `Parcode::load`, `execute_graph` (Default)
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_standard_file_io() -> parcode::Result<()> {
    let dir = tempfile::tempdir()?;
    let file_path = dir.path().join("std_io.par");
    let data = create_complex_data();

    Parcode::save(&file_path, &data)?;

    let loaded: ComplexData = Parcode::load(&file_path)?;

    assert_eq!(data, loaded);
    Ok(())
}

/// Pure Memory IO
/// Validate `Parcode::write`, `Parcode::load_bytes`, `DataSource::Memory`
#[test]
fn test_memory_io() -> parcode::Result<()> {
    let data = create_complex_data();
    let mut buffer = Vec::new();

    Parcode::write(&mut buffer, &data)?;

    assert!(!buffer.is_empty());

    let loaded: ComplexData = Parcode::load_bytes(buffer)?;

    assert_eq!(data, loaded);
    Ok(())
}

/// Explicit Synchronous Write (Forced)
/// Validate `Parcode::write_sync`, `execute_graph_sync`
#[test]
fn test_explicit_sync_write() -> parcode::Result<()> {
    let data = create_complex_data();
    let mut buffer = Vec::new();

    Parcode::write_sync(&mut buffer, &data)?;

    let loaded: ComplexData = Parcode::load_bytes(buffer)?;

    assert_eq!(data, loaded);
    Ok(())
}

/// Explicit Synchronous Write to File
/// Validate `Parcode::save_sync`
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_explicit_sync_save_file() -> parcode::Result<()> {
    let dir = tempfile::tempdir()?;
    let file_path = dir.path().join("sync_save.par");
    let data = SimpleData {
        id: 1,
        message: "Sync".into(),
    };

    Parcode::save_sync(&file_path, &data)?;
    let loaded: SimpleData = Parcode::load(&file_path)?;

    assert_eq!(data, loaded);
    Ok(())
}

/// Lazy Loading from Memory
/// Validate `Parcode::open_bytes`, `DataSource::Memory` with Lazy
#[test]
fn test_lazy_load_memory() -> parcode::Result<()> {
    let data = create_complex_data();
    let bytes = Parcode::serialize(&data)?;

    let file = Parcode::open_bytes(bytes)?;

    let root_mirror = file.root::<ComplexData>()?;

    assert_eq!(root_mirror.title, "Integration Test");

    let item_100 = root_mirror.numbers.get(100)?;
    assert_eq!(item_100, 100);

    Ok(())
}

/// Compression
/// Validate `ParcodeOptions`, `CompressorRegistry`
#[test]
fn test_compression_config() -> parcode::Result<()> {
    let data = create_complex_data();
    let mut buffer_compressed = Vec::new();
    let mut buffer_raw = Vec::new();

    Parcode::builder()
        .compression(true)
        .write(&mut buffer_compressed, &data)?;

    Parcode::write(&mut buffer_raw, &data)?;

    let loaded: ComplexData = Parcode::load_bytes(buffer_compressed)?;
    assert_eq!(data, loaded);

    Ok(())
}

/// Inspector
/// Validate `ParcodeInspector`, read metadata without deserializing
#[test]
fn test_inspector() -> parcode::Result<()> {
    let data = create_complex_data();
    let bytes = Parcode::serialize(&data)?;

    let report = Parcode::inspect_bytes(bytes)?;

    assert_eq!(report.global_version, 4);
    assert!(report.root_offset > 0);

    Ok(())
}
