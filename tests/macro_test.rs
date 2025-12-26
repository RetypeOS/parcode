#![allow(missing_docs)]

use parcode::{Parcode, ParcodeObject};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ParcodeObject)]
struct LevelState {
    id: u32,
    name: String,

    #[parcode(chunkable)]
    indices: Vec<u64>,
    #[cfg(feature = "lz4_flex")]
    #[parcode(chunkable, compression = "lz4")] // Explicit LZ4
    assets: Vec<u8>,
}

#[test]
fn test_macro_ergonomics() {
    let level = LevelState {
        id: 42,
        name: "Dungeon_01".into(),
        indices: (0..10_000).map(|i| i * 2).collect(),
        #[cfg(feature = "lz4_flex")]
        assets: vec![0xAA; 200_000], // 200KB
    };

    let file = NamedTempFile::new().expect("Failed to create temp file");

    Parcode::save(file.path(), &level).expect("Failed to save parcode data");

    let loaded: LevelState = Parcode::load(file.path()).expect("Failed to read parcode data");

    assert_eq!(level, loaded);
}
