use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    ScopedVectorSearch, SearchBudget, SearchError, SearchPage, SearchRequest, SearchScope,
    StrategyDescriptor,
};

/// Per-request services and authorization ceiling handed to a strategy.
/// Concrete dependencies owned by a strategy are supplied when it is
/// registered; the optional vector facade here is request-scoped so it cannot
/// accidentally search outside the caller's authorized graphs.
#[derive(Clone)]
pub struct SearchContext {
    scope: SearchScope,
    budget: SearchBudget,
    vectors: Option<Arc<dyn ScopedVectorSearch>>,
}

impl SearchContext {
    pub fn new(scope: SearchScope, budget: SearchBudget) -> Self {
        Self {
            scope,
            budget,
            vectors: None,
        }
    }

    pub fn with_vectors(mut self, vectors: Arc<dyn ScopedVectorSearch>) -> Self {
        self.vectors = Some(vectors);
        self
    }

    pub fn scope(&self) -> &SearchScope {
        &self.scope
    }

    pub fn budget(&self) -> &SearchBudget {
        &self.budget
    }

    pub fn vectors(&self) -> Result<&Arc<dyn ScopedVectorSearch>, SearchError> {
        self.vectors
            .as_ref()
            .ok_or_else(|| SearchError::Unsupported("no vector backend is configured".to_owned()))
    }
}

/// Object-safe extension point for classic, embedding, neural, hybrid, and
/// bounded agentic search strategies.
#[async_trait]
pub trait SearchStrategy: Send + Sync + 'static {
    fn descriptor(&self) -> &StrategyDescriptor;

    async fn search(
        &self,
        ctx: SearchContext,
        request: SearchRequest,
    ) -> Result<SearchPage, SearchError>;
}
