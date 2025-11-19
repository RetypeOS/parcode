//! PLACEHOLDER

use parcode::{
    graph::{TaskGraph, ChunkId, SerializationJob},
    visitor::ParcodeVisitor,
    format::ChildRef,
    Parcode,
    ParcodeReader,
    Result,
    ParcodeError, // Importante para el mapeo
};
use serde::{Serialize, Deserialize};
use tempfile::NamedTempFile;

// --- MOCK DATA STRUCTURES ---

// Wrapper para evitar la Orphan Rule sobre Vec<T>
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
struct ZoneList(Vec<GameZone>);

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
struct GameLevel {
    id: u32,
    name: String,
    zones: ZoneList, // Usamos el wrapper aquí
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
struct GameZone {
    zone_id: u32,
    data: Vec<u8>,
}

// --- IMPLEMENTACIONES ---

impl ParcodeVisitor for GameLevel {
    fn visit(&self, graph: &mut TaskGraph, parent_id: Option<ChunkId>) {
        let job = Box::new(self.clone());
        let my_id = graph.add_node(job);
        if let Some(pid) = parent_id { graph.link_parent_child(pid, my_id); }
        
        // Delegamos al wrapper
        self.zones.visit(graph, Some(my_id));
    }
    fn create_job(&self) -> Box<dyn SerializationJob> { Box::new(self.clone()) }
}

impl SerializationJob for GameLevel {
    fn execute(&self, _children: &[ChildRef]) -> Result<Vec<u8>> {
        // Serializamos solo la parte local
        let local_data = (&self.id, &self.name);
        bincode::serde::encode_to_vec(&local_data, bincode::config::standard())
            .map_err(|e| ParcodeError::Serialization(e.to_string())) // FIX: Mapeo manual
    }
    fn estimated_size(&self) -> usize { 100 }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

// Implementación sobre el Wrapper (ZoneList) en lugar de Vec directo
impl ParcodeVisitor for ZoneList {
    fn visit(&self, graph: &mut TaskGraph, parent_id: Option<ChunkId>) {
        let job = Box::new(self.clone());
        let my_id = graph.add_node(job);
        if let Some(pid) = parent_id { graph.link_parent_child(pid, my_id); }
        
        for zone in &self.0 {
            zone.visit(graph, Some(my_id));
        }
    }
    fn create_job(&self) -> Box<dyn SerializationJob> { Box::new(self.clone()) }
}

impl SerializationJob for ZoneList {
    fn execute(&self, _children: &[ChildRef]) -> Result<Vec<u8>> {
        Ok(Vec::new()) // Contenedor vacío
    }
    fn estimated_size(&self) -> usize { 0 }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

impl ParcodeVisitor for GameZone {
    fn visit(&self, graph: &mut TaskGraph, parent_id: Option<ChunkId>) {
        let job = Box::new(self.clone());
        let my_id = graph.add_node(job);
        if let Some(pid) = parent_id { graph.link_parent_child(pid, my_id); }
    }
    fn create_job(&self) -> Box<dyn SerializationJob> { Box::new(self.clone()) }
}

impl SerializationJob for GameZone {
    fn execute(&self, _children: &[ChildRef]) -> Result<Vec<u8>> {
        bincode::serde::encode_to_vec(self, bincode::config::standard())
            .map_err(|e| ParcodeError::Serialization(e.to_string())) // FIX: Mapeo manual
    }
    fn estimated_size(&self) -> usize { self.data.len() }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

// --- TEST ---

#[test]
fn test_full_lifecycle_write_read() -> Result<()> {
    let zones = ZoneList(vec![
        GameZone { zone_id: 1, data: vec![1, 2, 3] },
        GameZone { zone_id: 2, data: vec![4, 5, 6, 7, 8] },
        GameZone { zone_id: 3, data: vec![9; 100] },
    ]);
    
    let level = GameLevel {
        id: 99,
        name: "Test Level".to_string(),
        zones: zones.clone(),
    };

    let file = NamedTempFile::new().unwrap();
    let path = file.path();

    println!("-> Writing graph...");
    Parcode::save(path, &level)?;
    
    println!("-> Opening reader...");
    let reader = ParcodeReader::open(path)?;
    
    let root_node = reader.root()?;
    let (read_id, read_name): (u32, String) = root_node.deserialize_local()?;
    
    assert_eq!(read_id, level.id);
    assert_eq!(read_name, level.name);

    let children = root_node.children()?;
    assert_eq!(children.len(), 1);
    
    let vec_node = &children[0];
    let zone_nodes = vec_node.children()?;
    assert_eq!(zone_nodes.len(), 3);

    let zone_2_node = &zone_nodes[1];
    let loaded_zone: GameZone = zone_2_node.deserialize_local()?;
    
    assert_eq!(loaded_zone.zone_id, 2);
    assert_eq!(loaded_zone.data, vec![4, 5, 6, 7, 8]);

    Ok(())
}