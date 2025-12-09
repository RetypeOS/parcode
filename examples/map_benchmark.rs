// examples/map_benchmark.rs
//! Comprehensive Benchmark: HashMap Optimization vs Standard Serialization.
//! Run: cargo run --example map_benchmark --release --features lz4_flex

#![allow(missing_docs)]
#![allow(unsafe_code)]

use parcode::{Parcode, ParcodeObject, ParcodeReader};
use serde::{Deserialize, Serialize};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use tempfile::NamedTempFile;

// ============================================================================
// 1. MEMORY PROFILER
// ============================================================================

struct ProfilingAllocator<A: GlobalAlloc> {
    inner: A,
    allocated: AtomicUsize,
    peak: AtomicUsize,
}

impl<A: GlobalAlloc> ProfilingAllocator<A> {
    const fn new(inner: A) -> Self {
        Self {
            inner,
            allocated: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        }
    }
    fn reset(&self) {
        self.allocated.store(0, Ordering::SeqCst);
        self.peak.store(0, Ordering::SeqCst);
    }
    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
}

unsafe impl<A: GlobalAlloc> GlobalAlloc for ProfilingAllocator<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { self.inner.alloc(layout) };
        if !ptr.is_null() {
            let current = self.allocated.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            self.peak.fetch_max(current, Ordering::SeqCst);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.allocated.fetch_sub(layout.size(), Ordering::SeqCst);
        unsafe { self.inner.dealloc(ptr, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: ProfilingAllocator<System> = ProfilingAllocator::new(System);

// ============================================================================
// 2. DATA MODELS
// ============================================================================

// Mode A: Optimized Map (Bucket Sharding + Micro-Index)
#[derive(Serialize, Deserialize, ParcodeObject)]
struct OptimizedMap {
    #[parcode(map)]
    data: HashMap<u64, String>,
}

// Mode B: Standard Parcode (Single Chunk Blob)
#[derive(Serialize, Deserialize, ParcodeObject)]
struct StandardMap {
    #[parcode(chunkable)]
    data: HashMap<u64, String>,
}

// ============================================================================
// 3. BENCHMARK SUITE
// ============================================================================

fn main() -> parcode::Result<()> {
    println!("\n=== PARCODE MAP OPTIMIZATION AUDIT ===\n");

    let count = 500_000;
    print!("Generating dataset ({} items)... ", count);
    ALLOCATOR.reset();
    let mut map = HashMap::with_capacity(count);
    for i in 0..count {
        map.insert(
            i as u64,
            format!("payload_value_{}_extended_string_data", i),
        );
    }
    println!(
        "Done. Base RAM: {:.2} MB",
        ALLOCATOR.peak() as f64 / 1024.0 / 1024.0
    );

    let opt_data = OptimizedMap { data: map.clone() };
    let std_data = StandardMap { data: map.clone() };

    let file_opt = NamedTempFile::new().unwrap();
    let file_std = NamedTempFile::new().unwrap();
    let file_bin = NamedTempFile::new().unwrap();

    // ------------------------------------------------------------------------
    // PHASE 1: WRITE PERFORMANCE
    // ------------------------------------------------------------------------
    println!("\n[PHASE 1: WRITE]");

    // A. Optimized
    ALLOCATOR.reset();
    let start = Instant::now();
    Parcode::save(file_opt.path(), &opt_data)?;
    let dur = start.elapsed();
    let peak = ALLOCATOR.peak() as f64 / 1_048_576.0;
    let size_opt = file_opt.path().metadata()?.len() as f64 / 1_048_576.0;
    println!(
        "Parcode (Optimized): Time: {:.2?}, RAM Peak: {:.2} MB, Disk: {:.2} MB",
        dur, peak, size_opt
    );

    // B. Standard (Blob)
    ALLOCATOR.reset();
    let start = Instant::now();
    Parcode::save(file_std.path(), &std_data)?;
    let dur = start.elapsed();
    let peak = ALLOCATOR.peak() as f64 / 1_048_576.0;
    let size_std = file_std.path().metadata()?.len() as f64 / 1_048_576.0;
    println!(
        "Parcode (Standard):  Time: {:.2?}, RAM Peak: {:.2} MB, Disk: {:.2} MB",
        dur, peak, size_std
    );

    // C. Bincode
    ALLOCATOR.reset();
    let start = Instant::now();
    {
        let mut writer = BufWriter::new(File::create(file_bin.path())?);
        bincode::serde::encode_into_std_write(&map, &mut writer, bincode::config::standard())
            .unwrap();
    }
    let dur = start.elapsed();
    let peak = ALLOCATOR.peak() as f64 / 1_048_576.0;
    let size_bin = file_bin.path().metadata()?.len() as f64 / 1_048_576.0;
    println!(
        "Bincode (Native):    Time: {:.2?}, RAM Peak: {:.2} MB, Disk: {:.2} MB",
        dur, peak, size_bin
    );

    // ------------------------------------------------------------------------
    // PHASE 2: RANDOM ACCESS (1000 Lookups)
    // ------------------------------------------------------------------------
    println!("\n[PHASE 2: RANDOM ACCESS (1000 lookups)]");

    // A. Optimized (Lazy)
    ALLOCATOR.reset();
    let reader = ParcodeReader::open(file_opt.path())?;
    let lazy = reader.read_lazy::<OptimizedMap>()?;
    let start = Instant::now();
    let mut hits = 0;
    for i in 0..1000 {
        let key = (i * 12345) as u64 % count as u64;
        if lazy.data.get(&key)?.is_some() {
            hits += 1;
        }
    }
    let dur = start.elapsed();
    let peak = ALLOCATOR.peak() as f64 / 1_048_576.0;
    println!(
        "Parcode (Optimized): Time: {:.2?} ({:.2} µs/op), RAM Peak: {:.2} MB",
        dur,
        dur.as_micros() as f64 / 1000.0,
        peak
    );
    assert!(hits > 0);

    // B. Standard (Full Load required)
    // No podemos hacer random access sin cargar todo el blob
    ALLOCATOR.reset();
    let reader = ParcodeReader::open(file_std.path())?;
    let lazy = reader.read_lazy::<StandardMap>()?;
    let start = Instant::now();
    // Simulamos: Cargar todo 1 vez, luego 1000 lookups en memoria
    let loaded_map = lazy.data.load()?; // Coste masivo aquí
    for i in 0..1000 {
        let key = (i * 12345) as u64 % count as u64;
        black_box(loaded_map.get(&key));
    }
    let dur = start.elapsed();
    let peak = ALLOCATOR.peak() as f64 / 1_048_576.0;
    println!(
        "Parcode (Standard):  Time: {:.2?}, RAM Peak: {:.2} MB (Includes Full Load)",
        dur, peak
    );

    // C. Bincode (Full Load required)
    ALLOCATOR.reset();
    let start = Instant::now();
    let file = File::open(file_bin.path())?;
    let mut reader = BufReader::new(file);
    let loaded_map: HashMap<u64, String> =
        bincode::serde::decode_from_std_read(&mut reader, bincode::config::standard()).unwrap();
    for i in 0..1000 {
        let key = (i * 12345) as u64 % count as u64;
        black_box(loaded_map.get(&key));
    }
    let dur = start.elapsed();
    let peak = ALLOCATOR.peak() as f64 / 1_048_576.0;
    println!(
        "Bincode (Native):    Time: {:.2?}, RAM Peak: {:.2} MB (Includes Full Load)",
        dur, peak
    );

    Ok(())
}

fn black_box<T>(dummy: T) -> T {
    std::hint::black_box(dummy)
}
