use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    DocumentId, HydratedDocument, ScopedVectorSearch, SearchBudget, SearchError, SearchPage,
    SearchRequest, SearchScope, StrategyDescriptor,
};

/// Request-scoped access to authoritative search-result metadata. The
/// application binds authorization before exposing this facade; missing or
/// unauthorized documents are omitted from the returned collection.
#[async_trait]
pub trait ScopedDocumentHydrator: Send + Sync + 'static {
    async fn hydrate(
        &self,
        document_ids: Vec<DocumentId>,
    ) -> Result<Vec<HydratedDocument>, SearchError>;
}

/// Per-request services and authorization ceiling handed to a strategy.
/// Concrete dependencies owned by a strategy are supplied when it is
/// registered; the optional vector facade here is request-scoped so it cannot
/// accidentally search outside the caller's authorized graphs.
#[derive(Clone)]
pub struct SearchContext {
    scope: SearchScope,
    budget: SearchBudget,
    vectors: Option<Arc<dyn ScopedVectorSearch>>,
    documents: Option<Arc<dyn ScopedDocumentHydrator>>,
}

impl SearchContext {
    pub fn new(scope: SearchScope, budget: SearchBudget) -> Self {
        Self {
            scope,
            budget,
            vectors: None,
            documents: None,
        }
    }

    pub fn with_vectors(mut self, vectors: Arc<dyn ScopedVectorSearch>) -> Self {
        self.vectors = Some(vectors);
        self
    }

    pub fn with_documents(mut self, documents: Arc<dyn ScopedDocumentHydrator>) -> Self {
        self.documents = Some(documents);
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

    pub fn documents(&self) -> Result<&Arc<dyn ScopedDocumentHydrator>, SearchError> {
        self.documents.as_ref().ok_or_else(|| {
            SearchError::Unsupported("no search document hydrator is configured".to_owned())
        })
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
