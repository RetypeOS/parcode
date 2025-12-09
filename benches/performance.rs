// ===== benches\performance.rs =====
#![allow(missing_docs)]

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use parcode::ParcodeObject;
use parcode::{Parcode, ParcodeReader, graph::*, visitor::ParcodeVisitor};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::hint::black_box;
use std::io::BufWriter;
use tempfile::NamedTempFile;

// --- SETUP ---

#[derive(Clone, Serialize, Deserialize, ParcodeObject)]
struct BenchItem {
    id: u64,
    payload: Vec<u64>, // 1KB payload
}

// Wrapper to satisfy Parcode traits
#[derive(Clone, Serialize, Deserialize)]
struct BenchCollection(Vec<BenchItem>);

// Minimal Manual Implementation for Benchmarking
impl ParcodeVisitor for BenchCollection {
    fn visit<'a>(
        &'a self,
        graph: &mut TaskGraph<'a>,
        parent_id: Option<ChunkId>,
        config_override: Option<JobConfig>,
    ) {
        // Delegamos al Vec interno, propagando la configuración
        self.0.visit(graph, parent_id, config_override);
    }

    fn create_job<'a>(
        &'a self,
        config_override: Option<JobConfig>,
    ) -> Box<dyn SerializationJob<'a> + 'a> {
        let base = Box::new(ContainerJob);
        if let Some(cfg) = config_override {
            Box::new(parcode::rt::ConfiguredJob::new(base, cfg))
        } else {
            base
        }
    }
}

#[derive(Clone)]
struct ContainerJob;
impl SerializationJob<'_> for ContainerJob {
    fn execute(&self, _: &[parcode::format::ChildRef]) -> parcode::Result<Vec<u8>> {
        Ok(vec![])
    }
    fn estimated_size(&self) -> usize {
        0
    }
}

fn generate_data(count: usize) -> BenchCollection {
    let items = (0..count)
        .map(|i| BenchItem {
            id: i as u64,
            payload: vec![i as u64; 128], // ~1KB
        })
        .collect();
    BenchCollection(items)
}

// --- BENCHMARKS ---

fn bench_writers(c: &mut Criterion) {
    let item_count = 200_000;
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
                bincode::config::standard(),
            )
            .unwrap();
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
    let item_count = 200_000;

    println!("Readers Item count: {}", item_count);

    let data = generate_data(item_count);

    // Setup files
    let bincode_file = NamedTempFile::new().unwrap();
    bincode::serde::encode_into_std_write(
        &data.0,
        &mut BufWriter::new(&bincode_file),
        bincode::config::standard(),
    )
    .unwrap();
    let bincode_path = bincode_file.path().to_owned();

    let parcode_file = NamedTempFile::new().unwrap();
    Parcode::save(parcode_file.path(), &data).unwrap();
    let parcode_path = parcode_file.path().to_owned();

    let reader = ParcodeReader::open(&parcode_path).unwrap();
    let root = reader.root().unwrap();
    println!("Chunks detected: {}", root.children().unwrap().len());

    let mut group = c.benchmark_group("Deserialization Read");

    // 1. Bincode: Standard
    group.bench_function("bincode_read_all", |b| {
        b.iter(|| {
            let file = File::open(&bincode_path).unwrap();
            let _res: Vec<BenchItem> = bincode::serde::decode_from_std_read(
                &mut std::io::BufReader::new(file),
                bincode::config::standard(),
            )
            .unwrap();
        })
    });

    // 2. Parcode: Random Access (Item único)
    group.bench_function("parcode_random_access_10", |b| {
        b.iter(|| {
            let reader = ParcodeReader::open(&parcode_path).unwrap();
            let root = reader.root().unwrap();

            // Usamos la API de alto nivel get_item
            for i in (0..10).map(|x| x * (item_count / 10)) {
                let _obj: BenchItem = root.get(i).unwrap();
            }
        })
    });

    // 3. Parcode: Full Scan (Manual Shard Iteration)
    group.bench_function("parcode_full_scan_manual", |b| {
        b.iter(|| {
            let reader = ParcodeReader::open(&parcode_path).unwrap();
            let root = reader.root().unwrap();

            // Obtenemos los Shards (Hijos directos)
            let shards = root.children().unwrap();

            for shard_node in shards {
                // Deserializamos el Shard completo (Vec<BenchItem>)
                let items: Vec<BenchItem> = shard_node.decode().unwrap();

                // Iteramos los items en memoria (simulando uso)
                for item in items {
                    black_box(item);
                }
            }
        })
    });

    // 4. Parcode: Parallel Stitching (La nueva joya)
    // Añadimos esto para probar la velocidad de reconstrucción total
    group.bench_function("parcode_read_all_parallel", |b| {
        b.iter(|| {
            let reader = ParcodeReader::open(&parcode_path).unwrap();
            let root = reader.root().unwrap();
            // Reconstruye el Vec<BenchItem> completo usando todos los cores
            let _res: Vec<BenchItem> = root.decode_parallel_collection().unwrap();
        })
    });

    group.finish();
}

criterion_group!(benches, bench_writers, bench_readers);
criterion_main!(benches);
