# Parcode Roadmap & Technical Debt

This document tracks pending features, architectural optimizations, and API improvements planned for future versions of Parcode.

## 🟢 High Priority (Immediate - v0.4.x)

### 1. Serialization Backend Migration

* **Task:** Replace the direct dependency on `bincode` with `postcard`.
* **Reason:** Bincode is unmaintained, and Postcard offers VarInts (smaller size).
* **Action:** Implement `src/internal/serializer.rs` (Facade Pattern) to isolate the library.

### 2. Security and Stability Audit (Technical Debt)

* **Task:** Exhaustive review of `unsafe` blocks and error handling.
* **Actions:**
  * Validate "Parallel Stitching" in `reader.rs` with `Miri`.
  * Implement `cargo-fuzz` to test file corruption.
  * Ensure no formatting errors cause a `panic!`.

### 3. Code Generation Optimization (Macro Bloat)

* **Task:** Refactor the `ParcodeObject` macro.
* **Action:** Delegate the setup logic (`prepare_lazy_reader`) to `src/rt.rs` to reduce the size of the final binary.

---

## 🟡 Medium Priority (Collection Power)

Improvements to bring `ParcodeCollectionPromise` on par with native Rust vectors.

### 4. Enhanced Lazy Iterators

* **Task:**
  * `iter()`: Streaming iterator of items (deserializes one by one).
  * `iter_chunks()`: Iterator of raw payloads (useful for inspection/copying).

### 5. Range Access (Slicing)

* **Task:** Support `Range` syntax (e.g., `data.enemies.get_range(20..50)?`).
* **Challenge:** Efficient calculation of shard intersection using RLE.

### 6. Tolerant Access (Clamped Access)

* **Task:** Implement `get_range_clamped(..100)` / `get_some()`.
* **Behavior:** If the range exceeds the bounds, return the available elements instead of an error. Useful for UI/Pagination.

---

## 🔵 Low Priority / Research (Future)

### 7. "Auto" Adaptive Compression

* **Proposal:** `#[parcode(compression="auto")]`.
* **Logic:**
  * If not specified: `None` (Default).
  * If `"auto"`: The engine chooses at runtime (e.g., Zstd for floats, LZ4 for strings, None for high-entropy data).

### 8. Versioning Scheme (Schema Evolution)

* **Analysis:**
  * **Inline:** We delegate to `#[serde(default)]` (Works well).
  * **Structural:** If new `chunkable` fields are added, the graph changes. We need to investigate how to handle the absence of child nodes in old files without breaking the reading process.

### 9. Pure Asynchronous Backend (Experimental)

* **Goal:** Real `async/await` support with `tokio`/`async-std`.
* **Usage:** Direct reading from S3/HTTP without blocking threads. Requires deep redesign of the `Reader` (no mmap).
