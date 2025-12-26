#![allow(missing_docs)]
#[cfg(miri)]
mod miri_tests {
    use parcode::{Parcode, ParcodeObject};
    use serde::{Deserialize, Serialize};

    // Estructura simple para el test
    #[derive(Serialize, Deserialize, ParcodeObject, Debug, PartialEq, Clone)]
    struct DataChunk {
        id: u32,
        payload: Vec<u8>,
    }

    #[derive(Serialize, Deserialize, ParcodeObject, Debug, PartialEq)]
    struct World {
        #[parcode(chunkable)]
        chunks: Vec<DataChunk>,
    }

    #[test]
    fn test_parallel_reconstruction_safety() {
        // 1. Generar datos (Pequeños, para que Miri acabe rápido)
        // Usamos suficientes items para forzar la creación de múltiples shards
        // si la heurística lo permite, o forzamos shards manualmente si pudiéramos.
        // Con 100 items y overhead, debería ser suficiente para ejercitar el loop.
        let chunks: Vec<DataChunk> = (0..100)
            .map(|i| DataChunk {
                id: i,
                payload: vec![i as u8; 10], // Pequeño payload
            })
            .collect();

        let world = World {
            chunks: chunks.clone(),
        };

        // 2. Guardar en memoria (Buffer)
        let mut buffer = Vec::new();
        Parcode::builder()
            .write_to_writer(&mut buffer, &world)
            .expect("Write failed");

        // 3. Leer desde memoria (Simulando archivo)
        // Miri no soporta mmap ni File I/O real fácilmente.
        // TRUCO: ParcodeReader::open requiere un Path.
        // Para testear la lógica interna de `decode_parallel_collection` con Miri,
        // necesitamos un entorno que no use mmap si es posible, o usar archivos temporales.
        // Miri soporta File I/O básico con aislamiento.

        let path = "miri_test.par";
        std::fs::write(path, &buffer).expect("File write failed");

        // 4. Ejecutar la lectura (Aquí Miri vigilará cada puntero)
        let reader = Parcode::open(path).expect("Open failed");
        let world_mirror = reader.read_lazy::<World>().expect("Lazy read failed");

        // Esto dispara `decode_parallel_collection` y el bloque unsafe
        let loaded_chunks = world_mirror.chunks.load().expect("Load failed");

        // 5. Validar datos
        assert_eq!(loaded_chunks, chunks);

        // Cleanup
        std::fs::remove_file(path).unwrap();
    }
}
