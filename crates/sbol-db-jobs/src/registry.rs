use std::collections::HashMap;
use std::sync::Arc;

use crate::handler::{ErasedHandler, JobHandler, TypedHandler};

/// Maps `kind` strings to the [`ErasedHandler`] that knows how to run
/// them. The registry is frozen by the time the worker reads it; build
/// it once at startup and share the `Arc`.
#[derive(Default)]
pub struct JobRegistry {
    handlers: HashMap<&'static str, Arc<dyn ErasedHandler>>,
}

impl JobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<H: JobHandler>(mut self, handler: H) -> Self {
        let kind = handler.kind();
        let wrapped: Arc<dyn ErasedHandler> = Arc::new(TypedHandler { inner: handler });
        if self.handlers.insert(kind, wrapped).is_some() {
            tracing::warn!(kind, "duplicate job handler registration; replacing");
        }
        self
    }

    pub fn lookup(&self, kind: &str) -> Option<Arc<dyn ErasedHandler>> {
        self.handlers.get(kind).cloned()
    }

    pub fn kinds(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.handlers.keys().copied()
    }

    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}
