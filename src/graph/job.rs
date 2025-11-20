use crate::error::Result;
use crate::format::ChildRef;
use std::any::Any;

/// Configuración de ejecución para un nodo específico.
/// Se mantiene pequeño (1 byte) para paso eficiente por copia.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobConfig {
    /// ID del algoritmo de compresión.
    /// 0 = Sin Compresión (Default)
    /// 1 = Lz4 (si feature activa)
    /// 2..255 = Reservado
    pub compression_id: u8,
}

impl Default for JobConfig {
    fn default() -> Self {
        Self { compression_id: 0 }
    }
}

/// Represents a unit of work: a piece of data that knows how to serialize itself.
pub trait SerializationJob: Send + Sync {
    /// PLACEHOLDER
    fn execute(&self, children_refs: &[ChildRef]) -> Result<Vec<u8>>;
    /// PLACEHOLDER
    fn estimated_size(&self) -> usize;
    /// PLACEHOLDER
    fn as_any(&self) -> &dyn Any;

    /// Devuelve la configuración específica para este trabajo.
    /// Por defecto retorna configuración estándar (ID 0, sin flags).
    fn config(&self) -> JobConfig {
        JobConfig::default()
    }
}

impl std::fmt::Debug for Box<dyn SerializationJob> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SerializationJob(size={}, algo={})", self.estimated_size(), self.config().compression_id)
    }
}