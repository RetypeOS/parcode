// tests/lazy_test.rs

#![allow(missing_docs)]

use parcode::{Parcode, ParcodeObject, ParcodeReader};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

// --- ESTRUCTURAS ANIDADAS ---

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, ParcodeObject)]
struct Config {
    version: u32,
    #[parcode(chunkable)] // Nodo hijo
    blob: Vec<u8>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, ParcodeObject)]
struct Level {
    id: u64,
    name: String,

    #[parcode(chunkable)]
    config: Config, // Otro ParcodeObject

    #[parcode(chunkable)]
    geometry: Vec<u32>, // Colección
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

    let file = NamedTempFile::new().unwrap();
    Parcode::save(file.path(), &level).unwrap();

    let reader = ParcodeReader::open(file.path()).unwrap();

    // 1. Obtener el Espejo Lazy (NO carga los hijos chunkables)
    let lazy_level = reader.read_lazy::<Level>().unwrap();

    // 2. Validar Locales (Acceso directo, ya leídos)
    assert_eq!(lazy_level.id, 101);
    assert_eq!(lazy_level.name, "LazyZone");

    // 3. Validar Recursividad Lazy (Navegación sin carga total)
    // lazy_level.config es ConfigLazy
    // lazy_level.config.version es campo local de ConfigLazy (leído eager del chunk de config)
    // Para acceder a config, primero debemos "entrar" en su chunk.
    // La macro actual genera un método create_lazy que lee el payload.
    // PERO: El campo `config` en `LevelLazy` es de tipo `ConfigLazy`.
    // ¿Cuándo se instanció `ConfigLazy`?
    // En `Level::create_lazy` se llamó a `Config::create_lazy(child_node)`.
    // Por tanto, el payload de Config (que contiene version) YA SE LEYÓ.

    // Acceso directo a campo local del hijo:
    assert_eq!(lazy_level.config.version, 2);

    // 4. Validar Carga de Hoja Profunda
    // blob es ParcodeCollectionPromise<u8> dentro de config
    let loaded_blob = lazy_level.config.blob.load().unwrap();
    assert_eq!(loaded_blob.len(), 1024);
    assert_eq!(loaded_blob[0], 0xAA);

    // 5. Validar Acceso Parcial a Colección
    // geometry es ParcodeCollectionPromise<u32>
    let val = lazy_level.geometry.get(5000).unwrap();
    assert_eq!(val, 5000);
}
