//! Implementación de `ParcodeVisitor` para colecciones estándar de Rust.
//!
//! # Estrategia de Sharding V3: Adaptativa y Consciente de la Concurrencia
//!
//! Este módulo decide cómo dividir un `Vec<T>` en fragmentos (shards). La decisión se basa en:
//! 1. **Tamaño en Bytes:** Buscamos ~128KB por chunk para optimizar el throughput de SSD y la compresión.
//! 2. **Saturación de CPU:** Aseguramos crear suficientes chunks para alimentar todos los núcleos disponibles,
//!    incluso si eso implica crear chunks más pequeños (hasta un suelo de 4KB).
//! 3. **Muestreo de Datos Reales:** Medimos el tamaño de serialización real de una muestra para manejar
//!    datos alojados en el Heap (como `Vec<String>`) con precisión.

use crate::error::{ParcodeError, Result};
use crate::format::ChildRef;
use crate::graph::{ChunkId, JobConfig, SerializationJob, TaskGraph};
use crate::visitor::ParcodeVisitor;
use serde::{Deserialize, Serialize};

// --- CONSTANTES DE AJUSTE (TUNING) ---

/// Tamaño ideal para un chunk en disco. Optimizado para throughput de SSD.
const TARGET_SHARD_SIZE_BYTES: u64 = 128 * 1024; // 128 KB

/// Tamaño mínimo absoluto para evitar el overhead excesivo del sistema operativo/grafo.
const MIN_SHARD_SIZE_BYTES: u64 = 4 * 1024; // 4 KB

/// Multiplicador de núcleos de CPU. Si tenemos 8 núcleos, queremos al menos 32 tareas
/// para permitir un "work-stealing" y balanceo de carga efectivo en Rayon.
const TASKS_PER_CORE: usize = 4;

// --- Estructuras de Metadatos Internas ---

/// Estructura RLE (Run-Length Encoding) para mapear índices lógicos a shards físicos.
#[derive(Clone, Serialize, Deserialize, Debug)]
struct ShardRun {
    item_count: u32,
    repeat: u32,
}

/// El trabajo que serializa el Nodo Contenedor del Vector.
/// Contiene solo metadatos (tabla RLE y longitud total), no los datos en sí.
#[derive(Clone)]
struct VecContainerJob {
    shard_runs: Vec<ShardRun>,
    total_items: u64,
}

impl SerializationJob for VecContainerJob {
    fn execute(&self, _children_refs: &[ChildRef]) -> Result<Vec<u8>> {
        let mut buffer = Vec::new();
        // Escribir total_items (8 bytes)
        buffer.extend_from_slice(&self.total_items.to_le_bytes());
        
        // Escribir tabla RLE usando bincode
        let runs_bytes =
            bincode::serde::encode_to_vec(&self.shard_runs, bincode::config::standard())
                .map_err(|e| ParcodeError::Serialization(e.to_string()))?;
        buffer.extend_from_slice(&runs_bytes);
        
        Ok(buffer)
    }

    fn estimated_size(&self) -> usize {
        // Heurística simple: 8 bytes header + 8 bytes por run
        8 + (self.shard_runs.len() * 8)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// El trabajo que serializa un fragmento (Shard) de datos real.
/// Contiene un subconjunto del vector original (`data`).
#[derive(Clone)]
struct VecShardJob<T> {
    data: Vec<T>,
}

impl<T> SerializationJob for VecShardJob<T>
where
    T: Serialize + Send + Sync + 'static,
{
    fn execute(&self, _children_refs: &[ChildRef]) -> Result<Vec<u8>> {
        // Serialización "Quirúrgica": Convertimos el slice de datos a bytes usando Bincode.
        // Esto ocurre en paralelo en un hilo de trabajo.
        bincode::serde::encode_to_vec(&self.data, bincode::config::standard())
            .map_err(|e| ParcodeError::Serialization(e.to_string()))
    }

    fn estimated_size(&self) -> usize {
        self.data.len() * std::mem::size_of::<T>()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// --- Implementación del Visitante ---

impl<T> ParcodeVisitor for Vec<T>
where
    T: ParcodeVisitor + Clone + Send + Sync + 'static + Serialize,
{
    fn visit(&self, graph: &mut TaskGraph, parent_id: Option<ChunkId>, config_override: Option<JobConfig>) {
        let total_len = self.len();
        let items_per_shard;

        // --- FASE 1: CÁLCULO DE ESTRATEGIA DE SHARDING ---
        if total_len == 0 {
            items_per_shard = 1;
        } else {
            // 1. Medir coste de los datos (Sampling)
            // Tomamos hasta 8 elementos para estimar el tamaño real (útil para Strings/Heap).
            let sample_count = total_len.min(8);
            let sample_slice = &self[0..sample_count];

            let sample_size_bytes =
                match bincode::serde::encode_to_vec(sample_slice, bincode::config::standard()) {
                    Ok(vec) => vec.len() as u64,
                    Err(_) => 0,
                };

            // Calcular bytes promedio por item (mínimo 1 byte para evitar div por cero)
            let avg_item_size = if sample_size_bytes > 0 {
                (sample_size_bytes / sample_count as u64).max(1)
            } else {
                (std::mem::size_of::<T>() as u64).max(1)
            };

            // 2. Calcular Estrategias
            
            // Estrategia A: Optimizado para I/O (Llenar chunks de 128KB)
            let count_by_io = (TARGET_SHARD_SIZE_BYTES / avg_item_size).max(1) as usize;

            // Estrategia B: Optimizado para CPU (Llenar núcleos)
            // Queremos suficientes tareas para mantener a Rayon ocupado.
            let num_cpus = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1);
            let target_parallel_chunks = num_cpus * TASKS_PER_CORE;
            let count_by_cpu = (total_len / target_parallel_chunks).max(1);

            // 3. Fusión de Estrategias
            // Preferimos más chunks (CPU) a menos que sean ridículamente pequeños.
            let candidate_count = count_by_io.min(count_by_cpu);

            // Verificar tamaño físico del candidato
            let estimated_chunk_size = candidate_count as u64 * avg_item_size;

            if estimated_chunk_size < MIN_SHARD_SIZE_BYTES {
                // Demasiado pequeño. Escalar para cumplir el mínimo de 4KB.
                items_per_shard = (MIN_SHARD_SIZE_BYTES / avg_item_size).max(1) as usize;
            } else {
                items_per_shard = candidate_count;
            }
        }

        // --- FASE 2: CONSTRUCCIÓN DEL GRAFO ---

        // Generar slices (vistas) de los datos sin copiar memoria todavía.
        let chunks: Vec<&[T]> = self.chunks(items_per_shard).collect();

        // Construir metadatos RLE
        let mut shard_runs: Vec<ShardRun> = Vec::new();
        if !chunks.is_empty() {
            let mut current_run = ShardRun {
                item_count: chunks[0].len() as u32,
                repeat: 0,
            };

            for chunk in &chunks {
                let len = chunk.len() as u32;
                if len == current_run.item_count {
                    current_run.repeat += 1;
                } else {
                    shard_runs.push(current_run);
                    current_run = ShardRun {
                        item_count: len,
                        repeat: 1,
                    };
                }
            }
            shard_runs.push(current_run);
        }

        // 1. Crear y Registrar el Nodo Contenedor
        let container_inner = Box::new(VecContainerJob {
            shard_runs,
            total_items: self.len() as u64,
        });

        // Aplicar configuración (override) al contenedor si existe.
        // Si el usuario pide LZ4, el contenedor también se marca como LZ4 (aunque es pequeño).
        let container_job: Box<dyn SerializationJob> = if let Some(cfg) = config_override {
            Box::new(crate::rt::ConfiguredJob::new(container_inner, cfg))
        } else {
            container_inner
        };

        let my_id = graph.add_node(container_job);

        if let Some(pid) = parent_id {
            graph.link_parent_child(pid, my_id);
        }

        if self.is_empty() {
            return;
        }

        // 2. Crear Nodos Shard (Hijos)
        for chunk_slice in chunks {
            let shard_data = chunk_slice.to_vec();
            let shard_inner = Box::new(VecShardJob { data: shard_data });

            // PROPAGACIÓN DE CONFIGURACIÓN:
            // Es crítico aplicar la configuración del vector (ej. Compresión LZ4) a los Shards,
            // ya que es aquí donde residen el 99% de los bytes.
            let shard_job: Box<dyn SerializationJob> = if let Some(cfg) = config_override {
                Box::new(crate::rt::ConfiguredJob::new(shard_inner, cfg))
            } else {
                shard_inner
            };

            let shard_id = graph.add_node(shard_job);
            graph.link_parent_child(my_id, shard_id);

            // Recursión a elementos individuales.
            // Nota: Pasamos 'None' como config override.
            // Razón: Los items T se serializan dentro del payload del Shard usando Bincode.
            // No son nodos independientes del grafo (a menos que T cree explícitamente sub-nodos).
            // Si T es un struct complejo, su propia configuración (vía Macro) dictará cómo se comporta.
            for item in chunk_slice {
                item.visit(graph, Some(shard_id), None);
            }
        }
    }

    fn create_job(&self, config_override: Option<JobConfig>) -> Box<dyn SerializationJob> {
        // Este método se usa si el Vec es la raíz absoluta o para instanciación genérica.
        let inner = Box::new(VecContainerJob {
            shard_runs: Vec::new(),
            total_items: 0,
        });
        
        if let Some(cfg) = config_override {
            Box::new(crate::rt::ConfiguredJob::new(inner, cfg))
        } else {
            inner
        }
    }
}

// --- Implementación para Primitivos ---

#[derive(Clone)]
struct PrimitiveJob<T>(T);

impl<T> SerializationJob for PrimitiveJob<T>
where
    T: Serialize + Send + Sync + Clone + 'static,
{
    fn execute(&self, _: &[ChildRef]) -> Result<Vec<u8>> {
        bincode::serde::encode_to_vec(&self.0, bincode::config::standard())
            .map_err(|e| ParcodeError::Serialization(e.to_string()))
    }

    fn estimated_size(&self) -> usize {
        std::mem::size_of::<T>()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl<T: ParcodeVisitor> ParcodeVisitor for &T {
    fn visit(&self, graph: &mut TaskGraph, parent_id: Option<ChunkId>, config_override: Option<JobConfig>) {
        (**self).visit(graph, parent_id, config_override)
    }

    fn create_job(&self, config_override: Option<JobConfig>) -> Box<dyn SerializationJob> {
        (**self).create_job(config_override)
    }
}

/// Macro para implementar ParcodeVisitor en tipos primitivos masivamente.
macro_rules! impl_primitive_visitor {
    ($($t:ty),*) => {
        $(
            impl ParcodeVisitor for $t {
                fn visit(&self, graph: &mut TaskGraph, parent_id: Option<ChunkId>, config_override: Option<JobConfig>) {
                    // OPTIMIZACIÓN CRÍTICA:
                    // Si tenemos un padre (ej: estamos dentro de un Vec<u64>), NO creamos un nodo.
                    // Somos datos "inlined" dentro del payload del padre.
                    // Solo creamos nodo si somos la RAÍZ absoluta (parent_id es None).
                    if parent_id.is_none() {
                        let job = self.create_job(config_override);
                        graph.add_node(job);
                    }
                }

                fn create_job(&self, config_override: Option<JobConfig>) -> Box<dyn SerializationJob> {
                    let base_job = Box::new(PrimitiveJob(self.clone()));
                    
                    // Aplicamos la configuración (wrapper) si se solicita.
                    if let Some(cfg) = config_override {
                        Box::new(crate::rt::ConfiguredJob::new(base_job, cfg))
                    } else {
                        base_job
                    }
                }
            }
        )*
    }
}

// Aplicar a todos los tipos estándar soportados por Bincode/Serde.
impl_primitive_visitor!(
    u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, f32, f64, bool, String
);