#![allow(missing_docs)]

fn main() -> parcode::Result<()> {
    use parcode::{Parcode, ParcodeObject};
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;

    println!("=== PARCODE TORTURE TEST ===");

    // ------------------------------------------------------------------
    // SCENARIO 1: THE JAGGED VECTOR
    // ------------------------------------------------------------------
    println!("\n[Scenario 1] Jagged Vector (Tiny header, Massive body)");

    #[derive(Serialize, Deserialize, ParcodeObject, PartialEq, Debug, Clone)]
    struct JaggedContainer {
        #[parcode(chunkable)]
        data: Vec<String>,
    }

    let mut jagged_data = Vec::new();
    for i in 0..100 {
        jagged_data.push(format!("tiny_{}", i));
    }
    // Bomb: A 10MB String
    let huge_string = "X".repeat(10 * 1024 * 1024);
    jagged_data.push(huge_string.clone());
    for i in 0..100 {
        jagged_data.push(format!("tiny_tail_{}", i));
    }

    let jagged_obj = JaggedContainer { data: jagged_data };

    Parcode::save("stress_jagged.par", &jagged_obj)?;

    let report = Parcode::inspect("stress_jagged.par")?;
    println!("{}", report);

    // Verify Data
    let loaded_jagged: JaggedContainer = Parcode::load("stress_jagged.par")?;
    assert_eq!(loaded_jagged.data.len(), 201);
    assert_eq!(
        loaded_jagged.data.get(100).expect("Missing element").len(),
        10 * 1024 * 1024
    );
    println!(">> Jagged Integrity Verified!");

    // ------------------------------------------------------------------
    // SCENARIO 2: THE MATRYOSHKA
    // ------------------------------------------------------------------
    println!("\n[Scenario 2] Deep Nesting (Root -> Vec -> Struct -> Map -> Vec)");

    #[derive(Serialize, Deserialize, ParcodeObject, PartialEq, Debug, Clone)]
    struct DeepRoot {
        #[parcode(chunkable)]
        level_1_vec: Vec<Wrapper>,
    }

    #[derive(Serialize, Deserialize, ParcodeObject, PartialEq, Debug, Clone)]
    struct Wrapper {
        id: u32,
        #[parcode(map)]
        level_2_map: HashMap<String, DeepBlob>,
    }

    #[derive(Serialize, Deserialize, ParcodeObject, PartialEq, Debug, Clone)]
    struct DeepBlob {
        #[parcode(chunkable)]
        payload: Vec<u8>,
    }

    let mut map = HashMap::new();
    map.insert(
        "key_a".into(),
        DeepBlob {
            payload: vec![0xAA; 1024],
        },
    );
    map.insert(
        "key_b".into(),
        DeepBlob {
            payload: vec![0xBB; 1024],
        },
    );

    let root = DeepRoot {
        level_1_vec: vec![
            Wrapper {
                id: 1,
                level_2_map: map.clone(),
            },
            Wrapper {
                id: 2,
                level_2_map: map.clone(),
            },
        ],
    };

    Parcode::save("stress_nested.par", &root)?;

    let report_nested = Parcode::inspect("stress_nested.par")?;
    println!("{}", report_nested);

    // Verify Data
    let loaded_root: DeepRoot = Parcode::load("stress_nested.par")?;
    assert_eq!(loaded_root.level_1_vec.len(), 2);
    let first_wrapper = &loaded_root.level_1_vec.first().expect("Missing element");
    let blob = first_wrapper
        .level_2_map
        .get("key_a")
        .expect("Missing element");
    assert_eq!(blob.payload.len(), 1024);
    assert_eq!(blob.payload.first().expect("Missing element"), &0xAA);
    println!(">> Nested Integrity Verified!");

    std::fs::remove_file("stress_jagged.par")?;
    std::fs::remove_file("stress_nested.par")?;

    Ok(())
}
