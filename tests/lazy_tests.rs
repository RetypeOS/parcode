#![allow(missing_docs)]

use parcode::{Parcode, ParcodeObject, ParcodeReader};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, ParcodeObject)]
struct Config {
    version: u32,
    #[parcode(chunkable)]
    blob: Vec<u8>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, ParcodeObject)]
struct Level {
    id: u64,
    name: String,

    #[parcode(chunkable)]
    config: Config,

    #[parcode(chunkable)]
    geometry: Vec<u32>,
}

#[test]
fn test_lazy_mirror_access() {
    let config = Config {
        version: 2,
        blob: vec![0xAA; 1024],
    };
    let level = Level {
        id: 101,
        name: "LazyZone".into(),
        config: config.clone(),
        geometry: (0..10_000).collect(),
    };

    let file = NamedTempFile::new().expect("Failed to create temp file");
    Parcode::save(file.path(), &level).expect("Failed to save parcode data");

    let reader = ParcodeReader::open(file.path()).expect("Failed to open reader");

    let lazy_level = reader.read_lazy::<Level>().expect("Failed to read lazy");

    assert_eq!(lazy_level.id, 101);
    assert_eq!(lazy_level.name, "LazyZone");

    assert_eq!(lazy_level.config.version, 2);

    let loaded_blob = lazy_level.config.blob.load().expect("Failed to load blob");
    assert_eq!(loaded_blob.len(), 1024);
    assert_eq!(*loaded_blob.first().expect("Blob empty"), 0xAA);

    let val = lazy_level
        .geometry
        .get(5000)
        .expect("Failed to get geometry");
    assert_eq!(val, 5000);
}
