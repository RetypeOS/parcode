#![allow(missing_docs)]

use parcode::{Parcode, ParcodeObject, ParcodeReader};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tempfile::NamedTempFile;

// Estructura que usa el modo mapa optimizado
#[derive(Serialize, Deserialize, ParcodeObject)]
struct UserDatabase {
    id: u32,
    #[parcode(map, compression = "lz4")] // Activamos modo Mapa y LZ4
    users: HashMap<String, UserProfile>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, ParcodeObject)]
struct UserProfile {
    level: u32,
    score: u64,
}

#[test]
fn test_optimized_map_access() {
    // 1. Generar Datos (Suficientes para provocar sharding real)
    let mut users = HashMap::new();
    for i in 0..5000 {
        users.insert(
            format!("user_{}", i),
            UserProfile {
                level: i % 100,
                score: i as u64 * 10,
            },
        );
    }

    let db = UserDatabase {
        id: 1,
        users: users.clone(),
    };

    let file = NamedTempFile::new().expect("Failed to create temp file");
    Parcode::save(file.path(), &db).expect("Failed to save parcode data");

    // 2. Lectura Lazy con Acceso Aleatorio O(1)
    let reader = ParcodeReader::open(file.path()).expect("Failed to open reader");
    let lazy_db = reader
        .read_lazy::<UserDatabase>()
        .expect("Failed to read lazy");

    // A. Búsqueda Exitosa (Random Access)
    let target_key = "user_4242".to_string();
    let profile = lazy_db
        .users
        .get(&target_key)
        .expect("Failed to get user")
        .expect("User should exist");

    assert_eq!(profile.level, 4242 % 100);
    assert_eq!(profile.score, 42420);

    // B. Búsqueda Fallida (No existe)
    let missing = lazy_db
        .users
        .get(&"admin_root".to_string())
        .expect("Failed to get missing user");
    assert!(missing.is_none());

    // C. Carga Completa (Fallback a Vec<(K,V)> -> HashMap)
    // El método .load() devuelve el HashMap completo reconstruido
    let loaded_map = lazy_db.users.load().expect("Failed to load map");
    assert_eq!(loaded_map.len(), 5000);
    assert_eq!(
        loaded_map.get("user_100").expect("Key not found"),
        users.get("user_100").expect("Key not found")
    );
}
