//! Runtime utilities for generated code (Macros).
//! Do not use directly.

use crate::graph::{SerializationJob, JobConfig};
use crate::format::ChildRef;
use crate::Result;
use std::any::Any;

/// Wrapper que inyecta configuración a un Job existente.
/// Usado por la macro para aplicar atributos #[parcode(compression=...)]
#[derive(Debug)]
pub struct ConfiguredJob<J: ?Sized> {
    config: JobConfig,
    inner: Box<J>,
}

impl<J: SerializationJob + ?Sized> ConfiguredJob<J> {
    /// PLACEHOLDER
    pub fn new(inner: Box<J>, config: JobConfig) -> Self {
        Self { inner, config }
    }
}

impl<J: SerializationJob + ?Sized> SerializationJob for ConfiguredJob<J> {
    fn execute(&self, children_refs: &[ChildRef]) -> Result<Vec<u8>> {
        self.inner.execute(children_refs)
    }

    fn estimated_size(&self) -> usize {
        self.inner.estimated_size()
    }

    fn as_any(&self) -> &dyn Any {
        self.inner.as_any()
    }

    fn config(&self) -> JobConfig {
        self.config
    }
}