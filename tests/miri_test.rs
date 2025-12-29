//! MIRIFLAGS="-Zmiri-permissive-provenance -Zmiri-disable-stacked-borrows -Zmiri-disable-isolation"

#![allow(missing_docs)]
#[cfg(miri)]
mod miri_tests {
    use parcode::{Parcode, ParcodeObject};
    use serde::{Deserialize, Serialize};

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
        let chunks: Vec<DataChunk> = (0..100)
            .map(|i| DataChunk {
                id: i,
                payload: vec![i as u8; 10],
            })
            .collect();

        let world = World {
            chunks: chunks.clone(),
        };

        let mut buffer = Vec::new();
        Parcode::builder()
            .write(&mut buffer, &world)
            .expect("Write failed");

        let path = "miri_test.par";
        std::fs::write(path, &buffer).expect("File write failed");

        let reader: World = Parcode::load(path).expect("Load failed");

        assert_eq!(reader, world);

        let reader = Parcode::open(path).expect("Open failed");
        let world_mirror = reader.read_lazy::<World>().expect("Lazy read failed");

        let loaded_chunks = world_mirror.chunks.load().expect("Load failed");

        assert_eq!(loaded_chunks, chunks);

        // Cleanup
        std::fs::remove_file(path).unwrap();
    }
}
