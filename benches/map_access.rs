#![allow(missing_docs)]
use criterion::{Criterion, criterion_group, criterion_main};
use parcode::{Parcode, ParcodeObject};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tempfile::NamedTempFile;

#[derive(Serialize, Deserialize, ParcodeObject)]
struct MapContainer {
    #[parcode(map)] // Optimized
    opt_map: HashMap<u64, u64>,
    // #[parcode(chunkable)]
    // std_map: HashMap<u64, u64>,
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
        let file_handle = Parcode::open(&path).expect("Failed to open file");
        let lazy = file_handle
            .root::<MapContainer>()
            .expect("Failed to read lazy");

        b.iter(|| {
            let val = lazy.opt_map.get(&50_000).expect("Failed to get value");
            std::hint::black_box(val);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_map);
criterion_main!(benches);
