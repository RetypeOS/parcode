#![allow(missing_docs)]

use parcode::{Parcode, ParcodeObject};
use tempfile::NamedTempFile;

#[derive(ParcodeObject, Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
struct HeavyItem {
    id: u32,
    #[parcode(chunkable)]
    name: String,
    #[parcode(chunkable)]
    data: Vec<u8>,
}

#[derive(ParcodeObject, Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
struct Root {
    #[parcode(chunkable)]
    items: Vec<HeavyItem>,
}

#[test]
fn test_lazy_iterator_robust() {
    // 1. Setup data
    let mut items = Vec::new();
    for i in 0u32..100 {
        items.push(HeavyItem {
            id: i,
            name: format!("Item {}", i),
            data: vec![u8::try_from(i).expect("i fits in u8"); 10],
        });
    }
    let root = Root {
        items: items.clone(),
    };

    // 2. Serialize to temp file
    let temp_file = NamedTempFile::new().expect("Failed to create temp file");
    let path = temp_file.path();
    Parcode::save(path, &root).expect("Serialization failed");

    // 3. Load
    let file = Parcode::open(path).expect("File open failed");
    let loaded_root = file.root::<Root>().expect("Lazy load failed");
    let items_promise = &loaded_root.items;

    // 4. Test Collection Promise methods
    assert_eq!(items_promise.len(), 100);
    assert!(!items_promise.is_empty());

    // Test first() and last()
    let first_lazy = items_promise
        .first()
        .expect("first() failed")
        .expect("first() returned None");
    assert_eq!(first_lazy.id, 0);
    assert_eq!(
        first_lazy.name.load().expect("Failed to load name"),
        "Item 0"
    ); // Chunkable
    assert_eq!(first_lazy.data.len(), 10);

    let last_lazy = items_promise
        .last()
        .expect("last() failed")
        .expect("last() returned None");
    assert_eq!(last_lazy.id, 99);
    assert_eq!(
        last_lazy.name.load().expect("Failed to load name"),
        "Item 99"
    );
    assert_eq!(last_lazy.data.len(), 10);

    // 5. Test Iterator
    let mut iter = items_promise.iter_lazy().expect("iter_lazy() failed");

    // Initial state
    assert_eq!(iter.len(), 100);
    let (min, max) = iter.size_hint();
    assert_eq!(min, 100);
    assert_eq!(max, Some(100));

    // Iterate and check size_hint/len at each step
    for i in 0..100 {
        assert_eq!(iter.len(), 100 - i);
        let (min, max) = iter.size_hint();
        assert_eq!(min, 100 - i);
        assert_eq!(max, Some(100 - i));

        let item_lazy = iter
            .next()
            .expect("Iterator ended prematurely")
            .expect("Item load failed");
        assert_eq!(item_lazy.id, u32::try_from(i).expect("i fits in u32"));
        assert_eq!(
            item_lazy.name.load().expect("Failed to load name"),
            format!("Item {}", i)
        );
        assert_eq!(item_lazy.data.len(), 10);
    }

    // End of iteration
    assert_eq!(iter.len(), 0);
    assert!(iter.next().is_none());
}

#[test]
fn test_empty_collection_lazy() {
    let root = Root { items: Vec::new() };
    let temp_file = NamedTempFile::new().expect("Failed to create temp file");
    let path = temp_file.path();
    Parcode::save(path, &root).expect("Serialization failed");

    let file = Parcode::open(path).expect("File open failed");
    let loaded_root = file.root::<Root>().expect("Lazy load failed");
    let items_promise = &loaded_root.items;

    assert_eq!(items_promise.len(), 0);
    assert!(items_promise.is_empty());
    assert!(items_promise.first().expect("first() failed").is_none());
    assert!(items_promise.last().expect("last() failed").is_none());

    let mut iter = items_promise.iter_lazy().expect("iter_lazy() failed");
    assert_eq!(iter.len(), 0);
    assert!(iter.next().is_none());
}

#[test]
fn test_get_lazy_random_access() {
    let mut items = Vec::new();
    for i in 0u32..50 {
        items.push(HeavyItem {
            id: i,
            name: format!("Item {}", i),
            data: vec![u8::try_from(i).expect("i fits in u8"); 5],
        });
    }
    let root = Root { items };
    let temp_file = NamedTempFile::new().expect("Failed to create temp file");
    let path = temp_file.path();
    Parcode::save(path, &root).expect("Serialization failed");

    let file = Parcode::open(path).expect("File open failed");
    let loaded_root = file.root::<Root>().expect("Lazy load failed");
    let items_promise = &loaded_root.items;

    // Random access
    let indices = [0, 10, 25, 49];
    for &idx in &indices {
        let item_lazy = items_promise.get_lazy(idx).expect("get_lazy failed");
        assert_eq!(item_lazy.id, u32::try_from(idx).expect("idx fits in u32"));
        assert_eq!(
            item_lazy.name.load().expect("Failed to load name"),
            format!("Item {}", idx)
        );
        assert_eq!(item_lazy.data.len(), 5);
    }
}

#[test]
fn test_map_lazy_access() {
    use std::collections::HashMap;

    #[derive(ParcodeObject, Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
    struct Value {
        #[parcode(chunkable)]
        content: String,
    }

    #[derive(ParcodeObject, Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
    struct MapRoot {
        #[parcode(map)]
        data: HashMap<u32, Value>,
    }

    let mut data = HashMap::new();
    for i in 0..10 {
        data.insert(
            i,
            Value {
                content: format!("Value {}", i),
            },
        );
    }
    let root = MapRoot { data };

    let temp_file = NamedTempFile::new().expect("Failed to create temp file");
    let path = temp_file.path();
    Parcode::save(path, &root).expect("Serialization failed");

    let file = Parcode::open(path).expect("File open failed");
    let loaded_root = file.root::<MapRoot>().expect("Lazy load failed");
    let map_promise = &loaded_root.data;

    // Test get_lazy on map
    for i in 0..10 {
        let val_lazy = map_promise
            .get_lazy(&i)
            .expect("get_lazy failed")
            .expect("Value not found");
        assert_eq!(
            val_lazy.content.load().expect("Failed to load content"),
            format!("Value {}", i)
        );
    }

    // Test non-existent key
    assert!(
        map_promise
            .get_lazy(&100)
            .expect("get_lazy failed")
            .is_none()
    );
}
