#![allow(missing_docs)]

use parcode::{
    Parcode, ParcodeError, ParcodeReader, Result,
    format::ChildRef,
    graph::{ChunkId, SerializationJob, TaskGraph},
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
    fn visit(&self, graph: &mut TaskGraph, parent_id: Option<ChunkId>) {
        // 1. Crear trabajo (nodo hoja)
        let job = Box::new(self.clone());
        let my_id = graph.add_node(job);
        // 2. Ligar al padre
        if let Some(pid) = parent_id {
            graph.link_parent_child(pid, my_id);
        }
    }
    fn create_job(&self) -> Box<dyn SerializationJob> {
        Box::new(self.clone())
    }
}

// Implementación manual de SerializationJob (Bincode wrapper)
impl SerializationJob for TestUser {
    fn execute(&self, _children: &[ChildRef]) -> Result<Vec<u8>> {
        bincode::serde::encode_to_vec(self, bincode::config::standard())
            .map_err(|e| ParcodeError::Serialization(e.to_string()))
    }
    fn estimated_size(&self) -> usize {
        100
    } // Estimación burda
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// Implementación manual de ParcodeNative (Lectura)
impl ParcodeNative for TestUser {
    fn from_node(node: &ChunkNode<'_>) -> Result<Self> {
        node.decode::<Self>()
    }
}

// --- WRAPPER PARA VECTORES PERSONALIZADOS ---
// Necesario para simular una estructura que contiene un Vec, y delegar el visitado.

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
struct UserDirectory {
    region: String,
    users: Vec<TestUser>,
}

impl ParcodeVisitor for UserDirectory {
    fn visit(&self, graph: &mut TaskGraph, parent_id: Option<ChunkId>) {
        // 1. Nodo contenedor para 'region'
        // Nota: En una impl real, serializaríamos 'region' aquí y delegaríamos 'users' a los hijos.
        // Para simplificar el test, hacemos un nodo dummy que solo contiene la region,
        // y los usuarios cuelgan de él.

        #[derive(Serialize)]
        struct Header {
            region: String,
        }
        let header = Header {
            region: self.region.clone(),
        };

        // Job ad-hoc para el header
        struct HeaderJob(Header);
        impl SerializationJob for HeaderJob {
            fn execute(&self, _: &[ChildRef]) -> Result<Vec<u8>> {
                bincode::serde::encode_to_vec(&self.0, bincode::config::standard())
                    .map_err(|e| ParcodeError::Serialization(e.to_string()))
            }
            fn estimated_size(&self) -> usize {
                50
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }

        let my_id = graph.add_node(Box::new(HeaderJob(header)));
        if let Some(pid) = parent_id {
            graph.link_parent_child(pid, my_id);
        }

        // 2. DELEGAR AL VEC (Esto activa el Sharding Automático de Parcode)
        self.users.visit(graph, Some(my_id));
    }
    fn create_job(&self) -> Box<dyn SerializationJob> {
        panic!("Not used in root read")
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
    // Usamos la API de alto nivel que infiere tipos
    let loaded_user: TestUser = Parcode::read(file.path())?;

    assert_eq!(user, loaded_user);
    Ok(())
}

#[test]
fn test_massive_vector_sharding() -> Result<()> {
    // Objetivo: Crear suficientes datos para superar el umbral de 128KB y forzar múltiples shards.
    // u64 = 8 bytes. 100,000 items = 800KB. Debería crear al menos 6-7 shards.
    let count = 100_000;
    let data: Vec<u64> = (0..count).map(|i| i).collect();

    println!("Generating {} items (~{} KB)...", count, (count * 8) / 1024);

    let file = NamedTempFile::new().unwrap();

    // WRITE (Automatic Sharding en acción)
    Parcode::save(file.path(), &data)?;

    // READ (Automatic Parallel Stitching en acción)
    // Aquí es donde probamos la seguridad de threads y unsafe code corregido
    let loaded_data: Vec<u64> = Parcode::read(file.path())?;

    assert_eq!(data.len(), loaded_data.len());
    assert_eq!(data, loaded_data); // Verifica orden e integridad bit a bit

    // Verificación de bajo nivel: ¿Realmente se crearon shards?
    let reader = ParcodeReader::open(file.path())?;
    let root = reader.root()?;
    // El root de un Vec es el nodo contenedor. Sus hijos son los Shards.
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

    // WRITE
    // Nota: UserDirectory implementa Visitor delegando al Vec
    Parcode::save(file.path(), &dir)?;

    // READ MANUAL (Ya que UserDirectory no implementa ParcodeNative completo para este test mock)
    let reader = ParcodeReader::open(file.path())?;
    let root = reader.root()?;

    // 1. Leer Header Local
    #[derive(Deserialize)]
    struct Header {
        region: String,
    }
    let header: Header = root.decode()?;
    assert_eq!(header.region, "EU-West");

    // 2. Leer Hijos (El Vec<TestUser>)
    let children = root.children()?;
    assert_eq!(
        children.len(),
        1,
        "Debería haber 1 hijo: El nodo contenedor del Vec"
    );

    let vec_container = &children[0];

    // 3. Reconstruir Vec completo
    let loaded_users: Vec<TestUser> = vec_container.decode_parallel_collection()?;

    assert_eq!(dir.users, loaded_users);

    Ok(())
}

#[test]
fn test_random_access_logic() -> Result<()> {
    // Generamos un vector grande
    let count = 50_000;
    let data: Vec<u64> = (0..count).map(|i| i * 10).collect(); // 0, 10, 20...

    let file = NamedTempFile::new().unwrap();
    Parcode::save(file.path(), &data)?;

    let reader = ParcodeReader::open(file.path())?;
    let root = reader.root()?; // Root es el contenedor del Vec

    // Probamos acceso aleatorio sin cargar todo
    let val_0: u64 = root.get(0)?;
    let val_mid: u64 = root.get(25_000)?;
    let val_last: u64 = root.get(49_999)?;

    assert_eq!(val_0, 0);
    assert_eq!(val_mid, 250_000);
    assert_eq!(val_last, 499_990);

    // Probamos fuera de límites
    let err = root.get::<u64>(50_001);
    assert!(err.is_err());

    Ok(())
}

#[test]
fn test_corruption_and_errors() -> Result<()> {
    let file = NamedTempFile::new().unwrap();
    let path = file.path().to_owned();

    // CASO 1: Archivo vacío
    {
        let _f = File::create(&path).unwrap(); // Trucate a 0
    }
    let res = ParcodeReader::open(&path);
    assert!(
        matches!(res, Err(ParcodeError::Format(_))),
        "Debe fallar en archivo vacío"
    );

    // CASO 2: Magic Bytes inválidos
    {
        let mut f = File::create(&path).unwrap();
        // Escribimos basura suficiente para pasar el check de tamaño, pero mal magic
        let junk = vec![0u8; 100];
        f.write_all(&junk).unwrap();
    }
    let res = ParcodeReader::open(&path);
    if let Err(ParcodeError::Format(msg)) = res {
        assert!(
            msg.contains("Magic"),
            "Debe detectar magic bytes incorrectos"
        );
    } else {
        panic!("No detectó magic bytes inválidos");
    }

    // CASO 3: Truncado (Header válido, contenido cortado)
    // Escribimos un archivo válido primero
    let user = TestUser {
        id: 1,
        username: "test".into(),
        active: true,
    };
    Parcode::save(&path, &user)?;

    // Lo truncamos a la mitad
    let len = std::fs::metadata(&path).unwrap().len();
    let f = OpenOptions::new().write(true).open(&path).unwrap();
    f.set_len(len / 2).unwrap(); // Cortamos brutalmente

    // Intentamos leer. Open podría funcionar (lee el header al final, ups, si cortamos al final rompemos el header).
    // Si cortamos el header, open falla.
    // Si cortamos el medio, open funciona, pero read falla.

    // Al cortar con set_len, cortamos el final (donde está el header). Así que open debe fallar.
    let res = ParcodeReader::open(&path);
    assert!(res.is_err());

    Ok(())
}
