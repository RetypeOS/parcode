//! Verification test for .`get_lazy()` on collections.

use parcode::{Parcode, ParcodeObject, ParcodeReader};
use serde::{Deserialize, Serialize};

/// A heavy item that contains a large data payload.
#[derive(Serialize, Deserialize, ParcodeObject, Debug, Clone)]
struct HeavyItem {
    /// Unique identifier for the item.
    id: u32,
    /// Large data payload, stored in a separate chunk.
    #[parcode(chunkable)]
    data: Vec<u8>,
}

/// A container for a list of heavy items.
#[derive(Serialize, Deserialize, ParcodeObject, Debug)]
struct Container {
    /// List of items, stored as a chunkable collection.
    #[parcode(chunkable)]
    items: Vec<HeavyItem>,
}

/// Main entry point for the example.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = "lazy_collection_test.par";

    // 1. Create data
    let items: Vec<HeavyItem> = (0..10)
        .map(|i| HeavyItem {
            id: i,
            data: vec![u8::try_from(i).expect("Converson u64 to u8 has an error."); 1024], // 1KB data
        })
        .collect();
    let container = Container { items };

    Parcode::save(path, &container)?;

    // 2. Read back
    let reader = ParcodeReader::open(path)?;
    let root_lazy = reader.read_lazy::<Container>()?;

    // 3. Access lazy collection
    let items_lazy = root_lazy.items; // ParcodeCollectionPromise

    // 4. Test get_lazy(5)
    println!("Testing get_lazy(5)...");
    let item_lazy = items_lazy.get_lazy(5)?;

    // Verify local field access (should be instant, no I/O for data)
    println!("Item ID: {}", item_lazy.id);
    assert_eq!(item_lazy.id, 5);

    // Verify remote field is a promise
    // item_lazy.data is ParcodeCollectionPromise<u8> (Vec<u8>)

    println!("Loading item data...");
    let data = item_lazy.data.load()?;
    assert_eq!(data.len(), 1024);
    assert_eq!(data.first(), Some(&5));

    println!("Success!");

    std::fs::remove_file(path)?;
    Ok(())
}
