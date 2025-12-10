// examples/comprehensive_audit.rs
//! Comprehensive Architectural Audit: Parcode V3 vs Bincode.
//! Complex nested structures, mixed collections, no compression.
//! Run: cargo run --example audit --release

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
// 2. COMPLEX DATA MODEL (Deep Nesting)
// ============================================================================

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, ParcodeObject)]
struct WorldState {
    id: u64,
    name: String,

    // Test: O(1) Access in Map
    #[parcode(map)]
    users: HashMap<u64, UserProfile>,

    // Test: Lazy Navigation (Recursive)
    #[parcode(chunkable)]
    regions: Vec<Region>,

    // Test: Streaming/Partial Load
    #[parcode(chunkable)]
    system_logs: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, ParcodeObject)]
struct Region {
    id: u32,
    name: String,
    // Nested Chunkable: It allows loading a region without loading its zones.
    #[parcode(chunkable)]
    zones: Vec<Zone>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, ParcodeObject)]
struct Zone {
    id: u32,
    // Heavy payload
    #[parcode(chunkable)]
    terrain_data: Vec<u8>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
struct UserProfile {
    username: String,
    xp: u64,
    bio: String,
}

// ============================================================================
// 3. GENERATOR
// ============================================================================

fn generate_world() -> WorldState {
    print!("Generating Complex World... ");
    let start = Instant::now();

    // 1. Users Map (Heavy Map)
    let mut users = HashMap::new();
    for i in 0..100_000 {
        users.insert(
            i,
            UserProfile {
                username: format!("Player_{}", i),
                xp: i * 100,
                bio: "A very long description tailored to fill some bytes in the bucket.".into(),
            },
        );
    }

    // 2. Deep Hierarchy (World -> Regions -> Zones -> Data)
    let regions = (0..10)
        .map(|r_id| {
            Region {
                id: r_id,
                name: format!("Region_{}", r_id),
                zones: (0..50)
                    .map(|z_id| {
                        Zone {
                            id: z_id,
                            terrain_data: vec![r_id as u8; 20_000], // 20KB per zone * 50 = 1MB per region
                        }
                    })
                    .collect(),
            }
        })
        .collect();

    // 3. Flat Logs
    let logs = (0..50_000)
        .map(|i| format!("Log entry #{} with some content", i))
        .collect();

    let w = WorldState {
        id: 1,
        name: "Azeroth_V3".into(),
        users,
        regions,
        system_logs: logs,
    };

    println!(
        "Done in {:.2?}. RAM Footprint: {:.2} MB",
        start.elapsed(),
        ALLOCATOR.peak() as f64 / 1024.0 / 1024.0
    );
    w
}

// ============================================================================
// 4. AUDIT EXECUTION
// ============================================================================

fn main() -> parcode::Result<()> {
    println!("\n=== PARCODE V3 COMPREHENSIVE ARCHITECTURAL AUDIT ===\n");

    ALLOCATOR.reset();
    let world_data = generate_world();

    let path_par = NamedTempFile::new().unwrap();
    let path_bin = NamedTempFile::new().unwrap();

    // ------------------------------------------------------------------------
    // TEST 1: WRITING (Serialization)
    // ------------------------------------------------------------------------
    println!("\n[TEST 1: WRITING TO DISK]");

    // Parcode
    ALLOCATOR.reset();
    let t_start = Instant::now();
    Parcode::save(path_par.path(), &world_data)?;
    let t_par_write = t_start.elapsed();
    let mem_par_write = ALLOCATOR.peak();
    let size_par = path_par.path().metadata()?.len();

    // Bincode
    ALLOCATOR.reset();
    let t_start = Instant::now();
    {
        let mut w = BufWriter::new(File::create(path_bin.path())?);
        bincode::serde::encode_into_std_write(&world_data, &mut w, bincode::config::standard())
            .unwrap();
    }
    let t_bin_write = t_start.elapsed();
    let mem_bin_write = ALLOCATOR.peak();
    let size_bin = path_bin.path().metadata()?.len();

    print_metric("Write Time", t_par_write, t_bin_write);
    print_ram("Write RAM", mem_par_write, mem_bin_write);
    print_size("Disk Size", size_par, size_bin);

    // ------------------------------------------------------------------------
    // TEST 2: COLD START (Open File & Parse Metadata)
    // ------------------------------------------------------------------------
    println!("\n[TEST 2: COLD START (Ready to Read)]");

    // Parcode: Lazy Read (Solo lee Header + Root Chunk)
    ALLOCATOR.reset();
    let t_start = Instant::now();
    let reader = ParcodeReader::open(path_par.path())?;
    let lazy_world = reader.read_lazy::<WorldState>()?;
    let t_par_open = t_start.elapsed();
    let mem_par_open = ALLOCATOR.peak();

    // Bincode: Must deserialize EVERYTHING to be usable
    ALLOCATOR.reset();
    let t_start = Instant::now();
    let file = File::open(path_bin.path())?;
    let mut br = BufReader::new(file);
    let _full_world: WorldState =
        bincode::serde::decode_from_std_read(&mut br, bincode::config::standard()).unwrap();
    let t_bin_open = t_start.elapsed();
    let mem_bin_open = ALLOCATOR.peak();

    print_metric("Time to Ready", t_par_open, t_bin_open); // Expect massive win for Parcode
    print_ram("RAM to Ready", mem_par_open, mem_bin_open);

    // ------------------------------------------------------------------------
    // TEST 3: DEEP SURGICAL FETCH
    // Target: World -> Region[5] -> Zone[20] -> TerrainData (First byte)
    // ------------------------------------------------------------------------
    println!("\n[TEST 3: DEEP SURGICAL FETCH]");

    // Parcode: Navigate Graph
    ALLOCATOR.reset();
    let t_start = Instant::now();
    // 1. Get Region 5 Proxy (LazyCollection::get devuelve Region)
    let _region = lazy_world.regions.get(5)?;
    // 2. Region struct has 'zones' field which is LazyCollection<Zone> created via macro?
    // Wait, `Region` struct is: #[parcode(chunkable)] zones: Vec<Zone>.
    // The macro generated `RegionLazy` containing `zones: LazyCollection<Zone>`.
    // BUT `lazy_world.regions.get(5)` returns a `Region` (the full struct), NOT `RegionLazy`.
    // Why? Because `LazyCollection::get` returns `T` (deserialized).
    //
    // TO ACHIEVE TRUE DEEP LAZY:
    // We should not deserialize `Region`. We want a `RegionLazy`.
    // Current Implementation limitation: `LazyCollection<T>` deserializes T.
    // If T is `Region`, it deserializes `Region`.
    // Since `Region` has `zones` as chunkable, deserializing `Region` DOES NOT load `zones` data!
    // It only loads the `ChildRef` pointing to `zones`.
    // So `Region` is lightweight! It contains a `Vec` which the visitor implementation
    // reconstructs?
    // NO. If `Region` implements `ParcodeNative`, `from_node` will trigger.
    // Inside `from_node` for Region, `zones` (remote) calls `Vec::from_node`.
    // `Vec::from_node` loads the whole vector.
    //
    // AHA! To support deep lazy traversal, `LazyCollection::get` should ideally allow returning
    // a Lazy version if available. But currently returns `T`.
    //
    // HOWEVER: Even if it loads `Region`, does it load the content of `zones`?
    // Yes, standard `ParcodeNative` is recursive eager.
    //
    // TRUCO: Parcode is optimized. `Region` struct contains `Vec<Zone>`.
    // If we load `Region`, we load `Vec<Zone>`. `Zone` contains `Vec<u8>`.
    // Loading `Vec<Zone>` loads all zones.
    // This implies Test 3 will load Region 5 completely.
    // Compared to Bincode (Loaded EVERYTHING), it's still a win.

    let region_val = lazy_world.regions.get(5)?; // Loads Region 5 + All its Zones
    let zone_val = &region_val.zones[20];
    let byte = zone_val.terrain_data[0];

    let t_par_deep = t_start.elapsed();
    let mem_par_deep = ALLOCATOR.peak();
    black_box(byte);

    // Bincode: Already loaded in memory (zero time now, but paid huge upfront cost).
    // To be fair, we compare "Time to fetch specific item from cold disk".
    // Bincode Time = Open Time (Test 2) + Access Time (0).
    let t_bin_deep = t_bin_open;

    print_metric("Fetch Time", t_par_deep, t_bin_deep);
    print_ram("Fetch RAM", mem_par_deep, mem_bin_open);

    // ------------------------------------------------------------------------
    // TEST 4: MAP LOOKUP (Random Access)
    // Target: User #88888
    // ------------------------------------------------------------------------
    println!("\n[TEST 4: MAP LOOKUP (User #88888)]");

    // Parcode: Hash -> Bucket -> Scan
    ALLOCATOR.reset();
    let t_start = Instant::now();
    let user = lazy_world.users.get(&88888)?.expect("User not found");
    let t_par_map = t_start.elapsed();
    let mem_par_map = ALLOCATOR.peak();
    black_box(user);

    // Bincode: Memory lookup (Fast) but heavily taxed by initial load.
    // Again, comparing Cold Access Time.
    let t_bin_map = t_bin_open;

    print_metric("Lookup Time", t_par_map, t_bin_map);
    print_ram("Lookup RAM", mem_par_map, mem_bin_open);

    Ok(())
}

// --- UTILS ---

fn print_metric(label: &str, par: std::time::Duration, bin: std::time::Duration) {
    let p_ms = par.as_micros() as f64 / 1000.0;
    let b_ms = bin.as_micros() as f64 / 1000.0;
    let ratio = b_ms / p_ms;
    println!(
        "{:<15} | Parcode: {:>8.2} ms | Bincode: {:>8.2} ms | Speedup: {:>5.1}x",
        label, p_ms, b_ms, ratio
    );
}

fn print_ram(label: &str, par: usize, bin: usize) {
    let p_mb = par as f64 / 1048576.0;
    let b_mb = bin as f64 / 1048576.0;
    let savings = (1.0 - (p_mb / b_mb)) * 100.0;
    println!(
        "{:<15} | Parcode: {:>8.2} MB | Bincode: {:>8.2} MB | Savings: {:>5.1}%",
        label, p_mb, b_mb, savings
    );
}

fn print_size(label: &str, par: u64, bin: u64) {
    let p_mb = par as f64 / 1048576.0;
    let b_mb = bin as f64 / 1048576.0;
    let overhead = ((p_mb / b_mb) - 1.0) * 100.0;
    println!(
        "{:<15} | Parcode: {:>8.2} MB | Bincode: {:>8.2} MB | Overhead: {:>5.1}%",
        label, p_mb, b_mb, overhead
    );
}

fn black_box<T>(dummy: T) -> T {
    std::hint::black_box(dummy)
}
