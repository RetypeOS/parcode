//! benches/lazy_bench.rs

use criterion::{Criterion, criterion_group, criterion_main};
use parcode::{Parcode, ParcodeObject, ParcodeReader};
use serde::{Deserialize, Serialize};
use std::hint::black_box;
use tempfile::NamedTempFile;

#[derive(Serialize, Deserialize, Clone, ParcodeObject)]
struct HeavyNode {
    meta: u64,
    #[parcode(chunkable)]
    payload: Vec<u8>,
}

#[derive(Serialize, Deserialize, Clone, ParcodeObject)]
struct Root {
    #[parcode(chunkable)]
    child_a: HeavyNode,
    #[parcode(chunkable)]
    child_b: HeavyNode,
}

/// PLACEHOLDER
fn bench_lazy(c: &mut Criterion) {
    let data = Root {
        child_a: HeavyNode {
            meta: 1,
            payload: vec![0; 1_000_000],
        },
        child_b: HeavyNode {
            meta: 2,
            payload: vec![0; 1_000_000],
        },
    };

    let file = NamedTempFile::new().unwrap();
    Parcode::save(file.path(), &data).unwrap();
    let path = file.path().to_owned();

    let mut group = c.benchmark_group("Lazy Access");

    // Caso A: Carga Completa (Estándar)
    group.bench_function("full_load", |b| {
        b.iter(|| {
            let loaded: Root = Parcode::read(&path).unwrap();
            black_box(loaded.child_a.meta);
        })
    });

    // Caso B: Carga Lazy (Solo metadatos)
    group.bench_function("lazy_meta_only", |b| {
        b.iter(|| {
            let reader = ParcodeReader::open(&path).unwrap();
            let lazy = reader.read_lazy::<Root>().unwrap();
            // Accedemos a meta profundo A y B
            let sum = lazy.child_a.meta + lazy.child_b.meta;
            black_box(sum);
        })
    });

    // Caso C: Carga Parcial (Meta A + Payload A)
    group.bench_function("lazy_partial_load", |b| {
        b.iter(|| {
            let reader = ParcodeReader::open(&path).unwrap();
            let lazy = reader.read_lazy::<Root>().unwrap();
            let payload = lazy.child_a.payload.load().unwrap();
            black_box(payload.len());
        })
    });

    group.finish();
}

criterion_group!(benches, bench_lazy);
criterion_main!(benches);
