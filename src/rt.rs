//! Runtime utilities for generated code (Macros).
//! Do not use directly.

use crate::Result;
use crate::format::ChildRef;
use crate::graph::{JobConfig, SerializationJob};
use std::marker::PhantomData;

/// Wrapper that injects configuration into an existing Job.
#[derive(Debug)]
pub struct ConfiguredJob<'a, J: ?Sized> {
    config: JobConfig,
    inner: Box<J>,
    _marker: PhantomData<&'a ()>,
}

impl<'a, J: SerializationJob<'a> + ?Sized> ConfiguredJob<'a, J> {
    pub fn new(inner: Box<J>, config: JobConfig) -> Self {
        Self {
            inner,
            config,
            _marker: PhantomData,
        }
    }
}

impl<'a, J: SerializationJob<'a> + ?Sized> SerializationJob<'a> for ConfiguredJob<'a, J> {
    fn execute(&self, children_refs: &[ChildRef]) -> Result<Vec<u8>> {
        self.inner.execute(children_refs)
    }

    fn estimated_size(&self) -> usize {
        self.inner.estimated_size()
    }

    fn config(&self) -> JobConfig {
        self.config
    }
}
