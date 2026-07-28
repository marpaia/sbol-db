use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{DocumentId, SearchContext, SearchError, SearchRequest};

/// A lightweight candidate before primary-store hydration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub document_id: DocumentId,
    pub score: f32,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CandidateSet {
    pub source: String,
    pub items: Vec<Candidate>,
}

#[derive(Clone, Debug)]
pub struct CandidateRequest {
    pub search: SearchRequest,
    pub limit: usize,
}

/// Pluggable lexical, dense, sparse, graph, sequence, or algorithmic candidate
/// generator used to compose strategies.
#[async_trait]
pub trait CandidateSource: Send + Sync + 'static {
    fn id(&self) -> &str;

    async fn candidates(
        &self,
        ctx: SearchContext,
        request: CandidateRequest,
    ) -> Result<CandidateSet, SearchError>;
}

/// Deterministic combination of candidate rankings such as RRF or a weighted
/// normalized score.
pub trait Fusion: Send + Sync + 'static {
    fn id(&self) -> &str;
    fn fuse(&self, inputs: &[CandidateSet]) -> Result<CandidateSet, SearchError>;
}

#[derive(Clone, Debug)]
pub struct RerankRequest {
    pub query: String,
    pub candidates: CandidateSet,
}

/// Optional neural or algorithmic reranking stage.
#[async_trait]
pub trait Reranker: Send + Sync + 'static {
    fn id(&self) -> &str;
    async fn rerank(&self, request: RerankRequest) -> Result<CandidateSet, SearchError>;
}
