#![allow(missing_docs)]

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use parcode::{Parcode, ParcodeObject};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::hint::black_box;
use std::io::BufWriter;
use tempfile::NamedTempFile;

#[derive(Clone, Serialize, Deserialize, ParcodeObject, Debug)]
struct BenchItem {
    id: u64,
    #[parcode(chunkable)]
    payload: Vec<u64>,
}

#[derive(Clone, Serialize, Deserialize, ParcodeObject, Debug)]
struct BenchCollection {
    #[parcode(chunkable)]
    data: Vec<BenchItem>,
}

fn generate_data(count: usize) -> BenchCollection {
    let items = (0..count)
        .map(|i| BenchItem {
            id: i as u64,
            payload: vec![i as u64; 128], // ~1KB
        })
        .collect();
    BenchCollection { data: items }
}

// --- BENCHMARKS ---

fn bench_writers(c: &mut Criterion) {
    let item_count = 100_000;
    let data = generate_data(item_count);
    let raw_data = &data.data; // For bincode

    println!("Writers Item count: {}", item_count);

    let mut group = c.benchmark_group("Serialization Write");
    group.throughput(Throughput::Bytes((item_count * 1032) as u64));

    // 1. Baseline: Bincode (Single Threaded)
    group.bench_function("bincode_serialize", |b| {
        b.iter(|| {
            let file = NamedTempFile::new().expect("Failed to create temp file");
            let mut writer = BufWriter::new(file);
            bincode::serde::encode_into_std_write(
                black_box(raw_data),
                &mut writer,
                bincode::config::standard(),
            )
            .expect("Bincode serialization failed");
        });
    });

    // 2. Parcode
    group.bench_function("parcode_save", |b| {
        b.iter(|| {
            let file = NamedTempFile::new().expect("Failed to create temp file");
            Parcode::save(file.path(), black_box(&data)).expect("Failed to save parcode data");
        });
    });

    group.finish();
}

fn bench_readers(c: &mut Criterion) {
    let item_count = 100_000;

    println!("Readers Item count: {}", item_count);

    let data = generate_data(item_count);

    // Setup files
    let bincode_file = NamedTempFile::new().expect("Failed to create temp file");
    bincode::serde::encode_into_std_write(
        &data.data,
        &mut BufWriter::new(&bincode_file),
        bincode::config::standard(),
    )
    .expect("Bincode serialization failed");
    let bincode_path = bincode_file.path().to_owned();

    let parcode_file = NamedTempFile::new().expect("Failed to create temp file");
    Parcode::save(parcode_file.path(), &data).expect("Failed to save parcode data");
    let parcode_path = parcode_file.path().to_owned();

    let file_handle = Parcode::open(&parcode_path).expect("Failed to open file");
    let root = file_handle.root_node().expect("Failed to get root");
    println!(
        "Chunks detected: {}",
        root.children().expect("Failed to get children").len()
    );

    let mut group = c.benchmark_group("Deserialization Read");

    // 1. Bincode: Standard
    group.bench_function("bincode_read_all", |b| {
        b.iter(|| {
            let file = File::open(&bincode_path).expect("Failed to open file");
            let _res: Vec<BenchItem> = bincode::serde::decode_from_std_read(
                &mut std::io::BufReader::new(file),
                bincode::config::standard(),
            )
            .expect("Bincode deserialization failed");
        });
    });

    // 2. Parcode: Random Access (Single item)
    group.bench_function("parcode_random_access_10", |b| {
        b.iter(|| {
            let file_handle = Parcode::open(&parcode_path).expect("Failed to open file");
            let root = file_handle
                .root::<BenchCollection>()
                .expect("Failed to get root");

            for i in (0..10).map(|x| x * (item_count / 20)) {
                let _obj: BenchItem = root.data.get(i).expect("Failed to get item");
            }
        });
    });

    // 3. Parcode: Parallel Stitching
    group.bench_function("parcode_read_all_parallel", |b| {
        b.iter(|| {
            let _res: BenchCollection = Parcode::load(&parcode_path).expect("Failed to open file");
        });
    });

    // 4. Parcode: lazy iterator
    group.bench_function("parcode_read_all_iter", |b| {
        b.iter(|| {
            let file = Parcode::open(&parcode_path).expect("Failed to open file");
            let data = file
                .load_lazy::<BenchCollection>()
                .expect("Some error was ocurred");
            for item in data.data.iter_lazy().expect("Some error was ocurred") {
                let i = item.expect("Some error was ocurred");
                black_box(i);
            }
        });
    });

    group.finish();
}

criterion_group!(benches, bench_writers, bench_readers);
criterion_main!(benches);
