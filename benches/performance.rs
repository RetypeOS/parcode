#![allow(missing_docs)]

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use parcode::{Parcode, ParcodeReader, visitor::ParcodeVisitor, graph::*, ParcodeError};
use serde::{Serialize, Deserialize};
use std::fs::File;
use std::io::BufWriter;
use std::hint::black_box;
use tempfile::NamedTempFile;

// --- SETUP ---

#[derive(Clone, Serialize, Deserialize)]
struct BenchItem {
    id: u64,
    payload: Vec<u64>, // 1KB payload
}

// Wrapper to satisfy Parcode traits
#[derive(Clone, Serialize, Deserialize)]
struct BenchCollection(Vec<BenchItem>);

// Minimal Manual Implementation for Benchmarking
impl ParcodeVisitor for BenchCollection {
    fn visit(&self, graph: &mut TaskGraph, parent_id: Option<ChunkId>) {
        self.0.visit(graph, parent_id);
    }
    fn create_job(&self) -> Box<dyn SerializationJob> { 
        Box::new(ContainerJob) 
    }
}

impl ParcodeVisitor for BenchItem {
    fn visit(&self, _graph: &mut TaskGraph, _parent_id: Option<ChunkId>) {
        // BenchItem is a simple leaf (payload).
        // It does NOT create new child nodes.
        // Its data is serialized within the VecShardJob.
    }
    fn create_job(&self) -> Box<dyn SerializationJob> {
        // Not used if visit does not create nodes
        Box::new(ItemJob(self.clone()))
    }
}

#[derive(Clone)]
struct ContainerJob;
impl SerializationJob for ContainerJob {
    fn execute(&self, _: &[parcode::format::ChildRef]) -> parcode::Result<Vec<u8>> { Ok(vec![]) }
    fn estimated_size(&self) -> usize { 0 }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

#[derive(Clone)]
struct ItemJob(BenchItem);
impl SerializationJob for ItemJob {
    fn execute(&self, _: &[parcode::format::ChildRef]) -> parcode::Result<Vec<u8>> {
        bincode::serde::encode_to_vec(&self.0, bincode::config::standard())
            .map_err(|e| ParcodeError::Serialization(e.to_string()))
    }
    fn estimated_size(&self) -> usize { 1024 }
    fn as_any(&self) -> &dyn std::any::Any { self }
}


fn generate_data(count: usize) -> BenchCollection {
    let items = (0..count).map(|i| BenchItem {
        id: i as u64,
        payload: vec![i as u64; 128], // ~1KB
    }).collect();
    BenchCollection(items)
}

// --- BENCHMARKS ---

fn bench_writers(c: &mut Criterion) {
    let item_count = 100_000;
    let data = generate_data(item_count);
    let raw_data = &data.0; // For bincode

    println!("Writers Item count: {}", item_count);

    let mut group = c.benchmark_group("Serialization Write");
    group.throughput(Throughput::Bytes((item_count * 1032) as u64));

    // 1. Baseline: Bincode (Single Threaded)
    group.bench_function("bincode_serialize", |b| {
        b.iter(|| {
            let file = NamedTempFile::new().unwrap();
            let mut writer = BufWriter::new(file);
            bincode::serde::encode_into_std_write(
                black_box(raw_data), 
                &mut writer, 
                bincode::config::standard()
            ).unwrap();
        })
    });

    // 2. Parcode (Parallel Graph Engine)
    group.bench_function("parcode_save", |b| {
        b.iter(|| {
            let file = NamedTempFile::new().unwrap();
            Parcode::save(file.path(), black_box(&data)).unwrap();
        })
    });

    group.finish();
}

fn bench_readers(c: &mut Criterion) {
    let item_count = 100_000;
    let data = generate_data(item_count);

    println!("Readers Item count: {}", item_count);
    
    // Setup files
    let bincode_file = NamedTempFile::new().unwrap();
    bincode::serde::encode_into_std_write(&data.0, &mut BufWriter::new(&bincode_file), bincode::config::standard()).unwrap();
    let bincode_path = bincode_file.path().to_owned();

    let parcode_file = NamedTempFile::new().unwrap();
    Parcode::save(parcode_file.path(), &data).unwrap();
    let parcode_path = parcode_file.path().to_owned();

    let mut group = c.benchmark_group("Deserialization Read");

    // 1. Bincode: Must read EVERYTHING to get to the data
    group.bench_function("bincode_read_all", |b| {
        b.iter(|| {
             let file = File::open(&bincode_path).unwrap();
             let _res: Vec<BenchItem> = bincode::serde::decode_from_std_read(
                 &mut std::io::BufReader::new(file), 
                 bincode::config::standard()
             ).unwrap();
        })
    });

    // 2. Parcode: Lazy Load (Scenario: Read Root and access 10 random items)
    group.bench_function("parcode_random_access_10", |b| {
        b.iter(|| {
            let reader = ParcodeReader::open(&parcode_path).unwrap();
            let root = reader.root().unwrap();
            
            // Read 10 items spaced out
            for _ in (0..10).map(|x| x * (item_count/10)) {
                let _obj: BenchItem = root.get_child(9).unwrap().deserialize_local().unwrap();
            }
        })
    });
    
    // 3. Parcode: Full Scan (To measure overhead vs Bincode)
    group.bench_function("parcode_full_scan", |b| {
        b.iter(|| {
            let reader = ParcodeReader::open(&parcode_path).unwrap();
            let root = reader.root().unwrap();
            
            let items = root.children().unwrap();
            
            for node in items {
                let _obj: BenchItem = node.deserialize_local().unwrap();
            }
        })
    });

    group.finish();
}

criterion_group!(benches, bench_writers, bench_readers);
criterion_main!(benches);