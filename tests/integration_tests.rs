//! Integration test suite for Parcode.

#![allow(missing_docs)]

use parcode::{
    Parcode, ParcodeError, ParcodeObject, Result,
    format::ChildRef,
    graph::{ChunkId, JobConfig, SerializationJob, TaskGraph},
    visitor::ParcodeVisitor,
};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use tempfile::NamedTempFile;

// --- TEST INFRASTRUCTURE ---

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, ParcodeObject)]
struct TestUser {
    id: u64,
    username: String,
    active: bool,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
struct UserDirectory {
    region: String,
    users: Vec<TestUser>,
}

impl ParcodeVisitor for UserDirectory {
    fn visit<'a>(
        &'a self,
        graph: &mut TaskGraph<'a>,
        parent_id: Option<ChunkId>,
        config_override: Option<JobConfig>,
    ) {
        // 1. Container node for 'region'
        #[derive(Serialize)]
        struct Header {
            region: String,
        }
        let header = Header {
            region: self.region.clone(),
        };

        struct HeaderJob(Header);
        impl SerializationJob<'_> for HeaderJob {
            fn execute(&self, _: &[ChildRef]) -> Result<Vec<u8>> {
                bincode::serde::encode_to_vec(&self.0, bincode::config::standard())
                    .map_err(|e| ParcodeError::Serialization(e.to_string()))
            }
            fn estimated_size(&self) -> usize {
                50
            }
        }

        let header_job_base = Box::new(HeaderJob(header));
        // Apply config if it exists
        let header_job: Box<dyn SerializationJob<'_>> = if let Some(cfg) = config_override {
            Box::new(parcode::rt::ConfiguredJob::new(header_job_base, cfg))
        } else {
            header_job_base
        };

        let my_id = graph.add_node(header_job);
        if let Some(pid) = parent_id {
            graph.link_parent_child(pid, my_id);
        }

        // 2. DELEGATE TO VEC
        // Here we propagate None, but we could propagate config_override if we wanted the parent's
        // config to affect the child users.
        self.users.visit(graph, Some(my_id), None);
    }

    fn create_job(&self, _config_override: Option<JobConfig>) -> Box<dyn SerializationJob<'_>> {
        unreachable!("Not used in root read for UserDirectory mock")
    }
}

// --- TESTS ---

#[test]
fn test_primitive_lifecycle() -> Result<()> {
    let user = TestUser {
        id: 101,
        username: "satoshi".into(),
        active: true,
    };

    let file = NamedTempFile::new()?;

    // WRITE
    Parcode::save(file.path(), &user)?;

    // READ
    let loaded_user: TestUser = Parcode::load(file.path())?;

    assert_eq!(user, loaded_user);
    Ok(())
}

#[test]
fn test_massive_vector_sharding() -> Result<()> {
    let count: u64 = 100_000;
    let data: Vec<u64> = (0..count).collect();

    println!("Generating {} items (~{} KB)...", count, (count * 8) / 1024);

    let file = NamedTempFile::new()?;
    Parcode::save(file.path(), &data)?;

    let loaded_data: Vec<u64> = Parcode::load(file.path())?;

    assert_eq!(data.len(), loaded_data.len());
    assert_eq!(data, loaded_data);

    let file_handle = Parcode::open(file.path())?;
    let root = file_handle.root_node()?;
    let shards = root.children()?;
    println!("Shards created: {}", shards.len());
    assert!(
        shards.len() > 1,
        "The system should have fragmented the vector"
    );

    Ok(())
}

#[test]
fn test_nested_structures() -> Result<()> {
    let dir = UserDirectory {
        region: "EU-West".to_string(),
        users: vec![
            TestUser {
                id: 1,
                username: "a".into(),
                active: true,
            },
            TestUser {
                id: 2,
                username: "b".into(),
                active: false,
            },
            TestUser {
                id: 3,
                username: "c".into(),
                active: true,
            },
        ],
    };

    let file = NamedTempFile::new()?;
    Parcode::save(file.path(), &dir)?;

    let file_handle = Parcode::open(file.path())?;
    let root = file_handle.root_node()?;

    #[derive(Deserialize)]
    struct Header {
        region: String,
    }
    let header: Header = root.decode()?;
    assert_eq!(header.region, "EU-West");

    let children = root.children()?;
    assert_eq!(children.len(), 1);

    let vec_container = children.first().expect("No children found");
    let loaded_users: Vec<TestUser> = vec_container.decode_parallel_collection()?;

    assert_eq!(dir.users, loaded_users);

    Ok(())
}

#[test]
fn test_random_access_logic() -> Result<()> {
    let count: u64 = 50_000;
    let data: Vec<u64> = (0..count).map(|i| i * 10).collect();

    let file = NamedTempFile::new()?;
    Parcode::save(file.path(), &data)?;

    let file_handle = Parcode::open(file.path())?;
    let root = file_handle.root_node()?;

    let val_0: u64 = root.get(0)?;
    let val_mid: u64 = root.get(25_000)?;
    let val_last: u64 = root.get(49_999)?;

    assert_eq!(val_0, 0);
    assert_eq!(val_mid, 250_000);
    assert_eq!(val_last, 499_990);

    let err = root.get::<u64>(50_001);
    assert!(err.is_err());

    Ok(())
}

#[test]
fn test_corruption_and_errors() -> Result<()> {
    let file = NamedTempFile::new()?;
    let path = file.path().to_owned();

    {
        let _f = File::create(&path)?;
    }
    let res = Parcode::open(&path);
    assert!(matches!(res, Err(ParcodeError::Format(_))));

    {
        let mut f = File::create(&path)?;
        let junk = vec![0u8; 100];
        f.write_all(&junk)?;
    }
    let res = Parcode::open(&path);
    if let Err(ParcodeError::Format(msg)) = res {
        assert!(msg.contains("Magic"));
    } else {
        unreachable!("Did not detect invalid magic bytes");
    }

    let user = TestUser {
        id: 1,
        username: "test".into(),
        active: true,
    };
    Parcode::save(&path, &user)?;

    let len = std::fs::metadata(&path)?.len();
    let f = OpenOptions::new().write(true).open(&path)?;
    f.set_len(len / 2)?;

    let res = Parcode::open(&path);
    assert!(res.is_err());

    Ok(())
}
