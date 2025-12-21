//! Memory Profiling & Performance Example for Parcode V3.
//!
//! Run with: cargo run --example `memory_test` --release --features `lz4_flex`

#![allow(unsafe_code)]

use parcode::{
    Parcode,
    graph::{ChunkId, JobConfig, SerializationJob, TaskGraph},
    visitor::ParcodeVisitor,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::hint::black_box;
use std::io::{BufReader, BufWriter};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};
use tempfile::tempdir;

// ============================================================================
// 1. MEMORY ALLOCATOR PROFILER
// ============================================================================

/// A wrapper around the System allocator that tracks memory usage.
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

    fn current(&self) -> usize {
        self.allocated.load(Ordering::SeqCst)
    }
}

unsafe impl<A: GlobalAlloc> GlobalAlloc for ProfilingAllocator<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr: *mut u8;
        unsafe {
            ptr = self.inner.alloc(layout);
        }
        if !ptr.is_null() {
            let current = self.allocated.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            self.peak.fetch_max(current, Ordering::SeqCst);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.allocated.fetch_sub(layout.size(), Ordering::SeqCst);
        unsafe {
            self.inner.dealloc(ptr, layout);
        }
    }
}

#[global_allocator]
static ALLOCATOR: ProfilingAllocator<System> = ProfilingAllocator::new(System);

// ============================================================================
// 2. DATA STRUCTURES
// ============================================================================

/// A complex item with multiple fields to simulate real-world data.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, parcode::ParcodeObject)]
struct ComplexItem {
    /// Unique identifier.
    id: u64,
    /// Item name.
    name: String,
    /// Simulates binary blobs (textures, buffers).
    #[serde(with = "serde_bytes")]
    payload: Vec<u8>,
    /// Metadata tags.
    tags: HashMap<String, String>,
}

// --- CONFIGURATION WRAPPER ---
// Simulates usage of #[parcode(compression="lz4")] on a field.
// This wrapper forces LZ4 on the underlying data.

/// Wrapper to force LZ4 compression on the inner value.
struct Lz4Compressed<T>(pub T);

impl<T: ParcodeVisitor> ParcodeVisitor for Lz4Compressed<T> {
    fn visit<'a>(
        &'a self,
        graph: &mut TaskGraph<'a>,
        parent_id: Option<ChunkId>,
        _config_override: Option<JobConfig>,
    ) {
        // FORCE LZ4 Config (ID 1)
        let lz4_config = JobConfig {
            compression_id: 1,
            is_map: false,
        };

        // Delegate to inner
        self.0.visit(graph, parent_id, Some(lz4_config));
    }

    fn create_job<'a>(&'a self, _config: Option<JobConfig>) -> Box<dyn SerializationJob<'a> + 'a> {
        unreachable!("Wrapper should not create job directly in this example usage");
    }
}

// ============================================================================
// 3. GENERATOR
// ============================================================================

/// Generates a dataset of complex items.
fn generate_dataset(count: usize) -> Vec<ComplexItem> {
    print!(" -> Generating {} items... ", count);
    let start = Instant::now();
    let data = (0..count)
        .map(|i| {
            // Varied payload size to test fragmentation
            let size = (i % 1024) + 128;
            ComplexItem {
                id: i as u64,
                name: format!("asset_{:08}", i),
                payload: vec![u8::try_from((i * 3) % 255).expect("Value in range"); size],
                tags: (0..3)
                    .map(|j| (format!("meta_{}", j), format!("val_{}", i + j)))
                    .collect(),
            }
        })
        .collect();
    println!("Done in {:.2?}", start.elapsed());
    data
}

// ============================================================================
// 4. MAIN BENCHMARK
// ============================================================================

/// Main entry point for the memory benchmark.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    const ITEM_COUNT: usize = 200_000;
    // Approx size: 200k * ~600 bytes = ~120 MB raw data

    let dir = tempdir()?;
    let path_parcode_raw = dir.path().join("data_raw.par");
    let path_parcode_lz4 = dir.path().join("data_lz4.par");
    let path_bincode = dir.path().join("data.bin");

    println!("\n=== PARCODE V3 MEMORY & PERFORMANCE PROFILING ===\n");

    // 1. SETUP
    ALLOCATOR.reset();
    let data = generate_dataset(ITEM_COUNT);
    let gen_mem = ALLOCATOR.current();
    println!(
        " -> Dataset Memory Footprint: {:.2} MB\n",
        gen_mem as f64 / 1024.0 / 1024.0
    );

    // ------------------------------------------------------------------------
    // PHASE 1: WRITE
    // ------------------------------------------------------------------------
    println!("--- PHASE 1: WRITE ---");

    // A. Parcode (Default / No Compression)
    {
        ALLOCATOR.reset();
        let start = Instant::now();
        Parcode::save(&path_parcode_raw, &data)?;
        let dur = start.elapsed();
        let peak = ALLOCATOR.peak() as f64 / 1_048_576.0;
        let size = std::fs::metadata(&path_parcode_raw)?.len() as f64 / 1_048_576.0;

        println!(
            "[Parcode Raw]    Time: {:.2?}, Peak RAM: {:.2} MB, Disk: {:.2} MB",
            dur, peak, size
        );
    }

    // B. Parcode (Simulated LZ4 Config via Wrapper)
    {
        // Wrap the data to force LZ4 injection
        let wrapped_data = Lz4Compressed(&data);

        ALLOCATOR.reset();
        let start = Instant::now();

        // We save the wrapper. The wrapper's visit() forces LZ4 on the Vec.
        // Note: Parcode::save takes &T.
        Parcode::save(&path_parcode_lz4, &wrapped_data)?;

        let dur = start.elapsed();
        let peak = ALLOCATOR.peak() as f64 / 1_048_576.0;
        let size = std::fs::metadata(&path_parcode_lz4)?.len() as f64 / 1_048_576.0;

        println!(
            "[Parcode LZ4]    Time: {:.2?}, Peak RAM: {:.2} MB, Disk: {:.2} MB",
            dur, peak, size
        );
    }

    // C. Bincode (Baseline)
    {
        ALLOCATOR.reset();
        let start = Instant::now();
        let file = File::create(&path_bincode)?;
        let mut writer = BufWriter::new(file);
        bincode::serde::encode_into_std_write(&data, &mut writer, bincode::config::standard())?;
        let dur = start.elapsed();
        let peak = ALLOCATOR.peak() as f64 / 1_048_576.0;
        let size = std::fs::metadata(&path_bincode)?.len() as f64 / 1_048_576.0;

        println!(
            "[Bincode Baseline] Time: {:.2?}, Peak RAM: {:.2} MB, Disk: {:.2} MB",
            dur, peak, size
        );
    }

    // ------------------------------------------------------------------------
    // PHASE 2: FULL READ (Reconstruction)
    // ------------------------------------------------------------------------
    println!("\n--- PHASE 2: FULL PARALLEL READ ---");

    // A. Parcode
    {
        ALLOCATOR.reset();
        let start = Instant::now();
        let loaded: Vec<ComplexItem> = Parcode::load(&path_parcode_raw)?;
        black_box(loaded.len());
        let dur = start.elapsed();
        let peak = ALLOCATOR.peak() as f64 / 1_048_576.0;

        println!(
            "[Parcode Stitching] Time: {:.2?}, Peak RAM: {:.2} MB (Zero-Copy Assembly)",
            dur, peak
        );
        assert_eq!(loaded.len(), ITEM_COUNT);
    }

    // B. Bincode
    {
        ALLOCATOR.reset();
        let start = Instant::now();
        let file = File::open(&path_bincode)?;
        let mut reader = BufReader::new(file);
        let loaded: Vec<ComplexItem> =
            bincode::serde::decode_from_std_read(&mut reader, bincode::config::standard())?;
        black_box(loaded.len());
        let dur = start.elapsed();
        let peak = ALLOCATOR.peak() as f64 / 1_048_576.0;

        println!(
            "[Bincode Standard]  Time: {:.2?}, Peak RAM: {:.2} MB",
            dur, peak
        );
    }

    // ------------------------------------------------------------------------
    // PHASE 3: STREAMING (Iterative Read)
    // ------------------------------------------------------------------------
    println!("\n--- PHASE 3: STREAMING (Low Memory) ---");

    {
        ALLOCATOR.reset();
        let start = Instant::now();
        let file_handle = Parcode::open(&path_parcode_lz4)?; // Reading the compressed one!
        let root = file_handle.root_node()?;

        let mut count = 0;
        // Using the low-level iterator
        for item in root.iter::<ComplexItem>()? {
            let obj = item?;
            count += 1;
            black_box(obj.id);
        }

        let dur = start.elapsed();
        let peak = ALLOCATOR.peak() as f64 / 1_048_576.0;
        println!(
            "[Parcode Iterator]  Time: {:.2?}, Peak RAM: {:.2} MB (LZ4 Decompression on fly)",
            dur, peak
        );
        assert_eq!(count, ITEM_COUNT);
    }

    // ------------------------------------------------------------------------
    // PHASE 4: RANDOM ACCESS
    // ------------------------------------------------------------------------
    println!("\n--- PHASE 4: RANDOM ACCESS ---");

    {
        ALLOCATOR.reset();
        let start = Instant::now();
        let file_handle = Parcode::open(&path_parcode_raw)?;
        let root = file_handle.root_node()?;

        // Access 10 random items spread across the dataset
        for i in 0..10 {
            let idx = (i * 12345) % ITEM_COUNT;
            let item: ComplexItem = root.get(idx)?;
            assert_eq!(item.id, idx as u64);
        }

        let dur = start.elapsed();
        let peak = ALLOCATOR.peak() as f64 / 1_048_576.0;
        println!(
            "[Parcode Random]    Time: {:.2?}, Peak RAM: {:.2} MB (10 lookups)",
            dur, peak
        );
    }

    println!("\nDone.");
    Ok(())
}
