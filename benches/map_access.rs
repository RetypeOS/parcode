// benches/map_access.rs
//! PLACEHOLDER
//!
#![allow(missing_docs)]
use criterion::{Criterion, criterion_group, criterion_main};
use parcode::{Parcode, ParcodeObject, ParcodeReader};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tempfile::NamedTempFile;

#[derive(Serialize, Deserialize, ParcodeObject)]
struct MapContainer {
    /// PLACEHOLDER
    #[parcode(map)] // Optimized
    opt_map: HashMap<u64, u64>,
    // #[parcode(chunkable)] // Standard Blob (Unoptimized for random access)
    // std_map: HashMap<u64, u64>,
    // Nota: Para comparar justo, deberíamos usar otro campo o struct,
    // ya que HashMap sin 'map' flag se guarda como blob.
}

fn bench_map(c: &mut Criterion) {
    let count = 100_000;
    let mut map = HashMap::new();
    for i in 0..count {
        map.insert(i, i);
    }

    let data = MapContainer { opt_map: map };
    let file = NamedTempFile::new().expect("Failed to create temp file");
    Parcode::save(file.path(), &data).expect("Failed to save parcode data");
    let path = file.path().to_owned();

    let mut group = c.benchmark_group("Map Random Access");

    group.bench_function("optimized_lookup", |b| {
        // Setup reader once per batch to simulate persistent app
        let reader = ParcodeReader::open(&path).expect("Failed to open reader");
        let lazy = reader
            .read_lazy::<MapContainer>()
            .expect("Failed to read lazy");

        b.iter(|| {
            // Lookup key 50,000 (Middle)
            let val = lazy.opt_map.get(&50_000).expect("Failed to get value");
            std::hint::black_box(val);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_map);
criterion_main!(benches);
