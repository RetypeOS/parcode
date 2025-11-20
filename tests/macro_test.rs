#![allow(missing_docs)]

use parcode::{Parcode, ParcodeObject};
use serde::{Serialize, Deserialize};
use tempfile::NamedTempFile;

// Definición Ergonómica V3
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ParcodeObject)]
struct LevelState {
    id: u32,          // Local (Bincode directo)
    name: String,     // Local

    #[parcode(chunkable)] // Automatic settings
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
        assets: vec![0xAA; 200_000], // 200KB -> Forzará sharding + LZ4
    };

    let file = NamedTempFile::new().unwrap();
    
    // 1. Guardar (La macro se encarga de todo el grafo)
    Parcode::save(file.path(), &level).unwrap();

    // 2. Leer (La macro se encarga de la reconstrucción)
    let loaded: LevelState = Parcode::read(file.path()).unwrap();

    assert_eq!(level, loaded);
}