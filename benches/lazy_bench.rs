#![allow(missing_docs)]

use criterion::{Criterion, criterion_group, criterion_main};
use parcode::{Parcode, ParcodeObject, ParcodeReader};
use serde::{Deserialize, Serialize};
use std::hint::black_box;
use tempfile::NamedTempFile;

#[derive(Serialize, Deserialize, Clone, ParcodeObject)]
struct HeavyNode {
    /// Metadata field
    meta: u64,
    /// Heavy payload
    #[parcode(chunkable)]
    payload: Vec<u8>,
}

#[derive(Serialize, Deserialize, Clone, ParcodeObject)]
struct Root {
    /// First child
    #[parcode(chunkable)]
    child_a: HeavyNode,
    /// Second child
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

    let file = NamedTempFile::new().expect("Failed to create temp file");
    Parcode::save(file.path(), &data).expect("Failed to save parcode data");
    let path = file.path().to_owned();

    let mut group = c.benchmark_group("Lazy Access");

    // Caso A: Carga Completa (Estándar)
    group.bench_function("full_load", |b| {
        b.iter(|| {
            let loaded: Root = Parcode::read(&path).expect("Failed to read parcode data");
            black_box(loaded.child_a.meta);
        });
    });

    // Caso B: Carga Lazy (Solo metadatos)
    group.bench_function("lazy_meta_only", |b| {
        b.iter(|| {
            let reader = ParcodeReader::open(&path).expect("Failed to open reader");
            let lazy = reader.read_lazy::<Root>().expect("Failed to read lazy");
            // Accedemos a meta profundo A y B
            let sum = lazy.child_a.meta + lazy.child_b.meta;
            black_box(sum);
        });
    });

    // Caso C: Carga Parcial (Meta A + Payload A)
    group.bench_function("lazy_partial_load", |b| {
        b.iter(|| {
            let reader = ParcodeReader::open(&path).expect("Failed to open reader");
            let lazy = reader.read_lazy::<Root>().expect("Failed to read lazy");
            let payload = lazy.child_a.payload.load().expect("Failed to load payload");
            black_box(payload.len());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_lazy);
criterion_main!(benches);
