// ===== tests\integration_tests.rs =====
//! Suite de pruebas de integración para Parcode.

use parcode::{
    Parcode, ParcodeError, ParcodeReader, Result,
    format::ChildRef,
    graph::{ChunkId, JobConfig, SerializationJob, TaskGraph},
    reader::{ChunkNode, ParcodeNative},
    visitor::ParcodeVisitor,
};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use tempfile::NamedTempFile;

// --- INFRAESTRUCTURA DE PRUEBA (Mocks y Structs) ---

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
struct TestUser {
    id: u64,
    username: String,
    active: bool,
}

// Implementación manual de ParcodeVisitor (lo que haría la macro)
impl ParcodeVisitor for TestUser {
    fn visit<'a>(
        &'a self,
        graph: &mut TaskGraph<'a>,
        parent_id: Option<ChunkId>,
        config_override: Option<JobConfig>,
    ) {
        // 1. Crear trabajo
        let job = self.create_job(config_override);
        let my_id = graph.add_node(job);
        // 2. Ligar al padre
        if let Some(pid) = parent_id {
            graph.link_parent_child(pid, my_id);
        }
    }
    fn create_job<'a>(
        &'a self,
        config_override: Option<JobConfig>,
    ) -> Box<dyn SerializationJob<'a> + 'a> {
        let base = Box::new(self.clone());
        if let Some(cfg) = config_override {
            Box::new(parcode::rt::ConfiguredJob::new(base, cfg))
        } else {
            base
        }
    }
}

// Implementación manual de SerializationJob
impl SerializationJob<'_> for TestUser {
    fn execute(&self, _children: &[ChildRef]) -> Result<Vec<u8>> {
        bincode::serde::encode_to_vec(self, bincode::config::standard())
            .map_err(|e| ParcodeError::Serialization(e.to_string()))
    }
    fn estimated_size(&self) -> usize {
        100
    }
}

impl ParcodeNative for TestUser {
    fn from_node(node: &ChunkNode<'_>) -> Result<Self> {
        node.decode::<Self>()
    }
}

// --- WRAPPER PARA VECTORES PERSONALIZADOS ---

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
        // 1. Nodo contenedor para 'region'
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
        // Aplicamos config si existe
        let header_job: Box<dyn SerializationJob<'_>> = if let Some(cfg) = config_override {
            Box::new(parcode::rt::ConfiguredJob::new(header_job_base, cfg))
        } else {
            header_job_base
        };

        let my_id = graph.add_node(header_job);
        if let Some(pid) = parent_id {
            graph.link_parent_child(pid, my_id);
        }

        // 2. DELEGAR AL VEC
        // Aquí propagamos None, pero podríamos propagar config_override si quisiéramos que la config
        // del padre afectara a los hijos users.
        self.users.visit(graph, Some(my_id), None);
    }

    fn create_job(&self, _config_override: Option<JobConfig>) -> Box<dyn SerializationJob<'_>> {
        panic!("Not used in root read for UserDirectory mock")
    }
}

// --- MOCK DATA STRUCTURES (Game Level) ---

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
struct ZoneList(Vec<GameZone>);

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
struct GameLevel {
    id: u32,
    name: String,
    zones: ZoneList,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
struct GameZone {
    zone_id: u32,
    data: Vec<u8>,
}

// --- IMPLEMENTACIONES JUEGO ---

impl ParcodeVisitor for GameLevel {
    fn visit<'a>(
        &'a self,
        graph: &mut TaskGraph<'a>,
        parent_id: Option<ChunkId>,
        config_override: Option<JobConfig>,
    ) {
        let job = self.create_job(config_override);
        let my_id = graph.add_node(job);
        if let Some(pid) = parent_id {
            graph.link_parent_child(pid, my_id);
        }

        self.zones.visit(graph, Some(my_id), None);
    }
    fn create_job<'a>(
        &'a self,
        config_override: Option<JobConfig>,
    ) -> Box<dyn SerializationJob<'a> + 'a> {
        let base = Box::new(self.clone());
        if let Some(cfg) = config_override {
            Box::new(parcode::rt::ConfiguredJob::new(base, cfg))
        } else {
            base
        }
    }
}

impl SerializationJob<'_> for GameLevel {
    fn execute(&self, _children: &[ChildRef]) -> Result<Vec<u8>> {
        let local_data = (&self.id, &self.name);
        bincode::serde::encode_to_vec(&local_data, bincode::config::standard())
            .map_err(|e| ParcodeError::Serialization(e.to_string()))
    }
    fn estimated_size(&self) -> usize {
        100
    }
}

impl ParcodeVisitor for ZoneList {
    fn visit<'a>(
        &'a self,
        graph: &mut TaskGraph<'a>,
        parent_id: Option<ChunkId>,
        config_override: Option<JobConfig>,
    ) {
        let job = self.create_job(config_override);
        let my_id = graph.add_node(job);
        if let Some(pid) = parent_id {
            graph.link_parent_child(pid, my_id);
        }

        for zone in &self.0 {
            zone.visit(graph, Some(my_id), None);
        }
    }
    fn create_job<'a>(
        &'a self,
        config_override: Option<JobConfig>,
    ) -> Box<dyn SerializationJob<'a> + 'a> {
        let base = Box::new(self.clone());
        if let Some(cfg) = config_override {
            Box::new(parcode::rt::ConfiguredJob::new(base, cfg))
        } else {
            base
        }
    }
}

impl SerializationJob<'_> for ZoneList {
    fn execute(&self, _children: &[ChildRef]) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }
    fn estimated_size(&self) -> usize {
        0
    }
}

impl ParcodeVisitor for GameZone {
    fn visit<'a>(
        &'a self,
        graph: &mut TaskGraph<'a>,
        parent_id: Option<ChunkId>,
        config_override: Option<JobConfig>,
    ) {
        let job = self.create_job(config_override);
        let my_id = graph.add_node(job);
        if let Some(pid) = parent_id {
            graph.link_parent_child(pid, my_id);
        }
    }
    fn create_job<'a>(
        &'a self,
        config_override: Option<JobConfig>,
    ) -> Box<dyn SerializationJob<'a> + 'a> {
        let base = Box::new(self.clone());
        if let Some(cfg) = config_override {
            Box::new(parcode::rt::ConfiguredJob::new(base, cfg))
        } else {
            base
        }
    }
}

impl SerializationJob<'_> for GameZone {
    fn execute(&self, _children: &[ChildRef]) -> Result<Vec<u8>> {
        bincode::serde::encode_to_vec(self, bincode::config::standard())
            .map_err(|e| ParcodeError::Serialization(e.to_string()))
    }
    fn estimated_size(&self) -> usize {
        self.data.len()
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

    let file = NamedTempFile::new().unwrap();

    // WRITE
    Parcode::save(file.path(), &user)?;

    // READ
    let loaded_user: TestUser = Parcode::read(file.path())?;

    assert_eq!(user, loaded_user);
    Ok(())
}

#[test]
fn test_massive_vector_sharding() -> Result<()> {
    let count = 100_000;
    let data: Vec<u64> = (0..count).map(|i| i as u64).collect();

    println!("Generating {} items (~{} KB)...", count, (count * 8) / 1024);

    let file = NamedTempFile::new().unwrap();
    Parcode::save(file.path(), &data)?;

    let loaded_data: Vec<u64> = Parcode::read(file.path())?;

    assert_eq!(data.len(), loaded_data.len());
    assert_eq!(data, loaded_data);

    let reader = ParcodeReader::open(file.path())?;
    let root = reader.root()?;
    let shards = root.children()?;
    println!("Shards created: {}", shards.len());
    assert!(
        shards.len() > 1,
        "El sistema debería haber fragmentado el vector"
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

    let file = NamedTempFile::new().unwrap();
    Parcode::save(file.path(), &dir)?;

    let reader = ParcodeReader::open(file.path())?;
    let root = reader.root()?;

    #[derive(Deserialize)]
    struct Header {
        region: String,
    }
    let header: Header = root.decode()?;
    assert_eq!(header.region, "EU-West");

    let children = root.children()?;
    assert_eq!(children.len(), 1);

    let vec_container = &children[0];
    let loaded_users: Vec<TestUser> = vec_container.decode_parallel_collection()?;

    assert_eq!(dir.users, loaded_users);

    Ok(())
}

#[test]
fn test_random_access_logic() -> Result<()> {
    let count = 50_000;
    let data: Vec<u64> = (0..count).map(|i| i as u64 * 10).collect();

    let file = NamedTempFile::new().unwrap();
    Parcode::save(file.path(), &data)?;

    let reader = ParcodeReader::open(file.path())?;
    let root = reader.root()?;

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
    let file = NamedTempFile::new().unwrap();
    let path = file.path().to_owned();

    {
        let _f = File::create(&path).unwrap();
    }
    let res = ParcodeReader::open(&path);
    assert!(matches!(res, Err(ParcodeError::Format(_))));

    {
        let mut f = File::create(&path).unwrap();
        let junk = vec![0u8; 100];
        f.write_all(&junk).unwrap();
    }
    let res = ParcodeReader::open(&path);
    if let Err(ParcodeError::Format(msg)) = res {
        assert!(msg.contains("Magic"));
    } else {
        panic!("No detectó magic bytes inválidos");
    }

    let user = TestUser {
        id: 1,
        username: "test".into(),
        active: true,
    };
    Parcode::save(&path, &user)?;

    let len = std::fs::metadata(&path).unwrap().len();
    let f = OpenOptions::new().write(true).open(&path).unwrap();
    f.set_len(len / 2).unwrap();

    let res = ParcodeReader::open(&path);
    assert!(res.is_err());

    Ok(())
}
