//! Public contracts for extending sbol-db search.
//!
//! This crate deliberately contains no storage-engine, vector-database, model,
//! or HTTP dependencies. A strategy crate can depend on this SDK without
//! inheriting Tantivy, Qdrant, SQLx, FAISS, or an inference runtime. The sbol-db
//! application assembles concrete implementations and freezes the registries at
//! startup.

mod capability;
mod composition;
mod embedding;
mod error;
mod registry;
mod strategy;
mod types;
mod vector;

pub use capability::{
    DataEgress, FilterCapability, FilterKind, PaginationCapability, SearchInputKind,
    StrategyCapabilities, StrategyDescriptor, StrategyRequirements, TotalCapability,
};
pub use composition::{
    Candidate, CandidateRequest, CandidateSet, CandidateSource, Fusion, RerankRequest, Reranker,
};
pub use embedding::{
    EmbeddingBatch, EmbeddingDescriptor, EmbeddingInput, EmbeddingInputKind, EmbeddingOutput,
    EmbeddingProvider, EmbeddingVector, Normalization,
};
pub use error::{RegistrationError, SearchError, VectorError};
pub use registry::{
    EmbeddingRegistry, EmbeddingRegistryBuilder, StrategyRegistry, StrategyRegistryBuilder,
    VectorBackendRegistry, VectorBackendRegistryBuilder,
};
pub use strategy::{SearchContext, SearchStrategy};
pub use types::{
    DocumentId, Evidence, ExecutionMetadata, PageRequest, PredicateFilter, ScoreKind, SearchBudget,
    SearchFilters, SearchHit, SearchInput, SearchOptions, SearchPage, SearchRequest, SearchScope,
    StrategyRef, Total,
};
pub use vector::{
    ApplyReceipt, DistanceMetric, GenerationHandle, IndexGenerationSpec, ScopedVectorSearch,
    SnapshotRef, SparseVector, VectorBackend, VectorBackendDescriptor, VectorCapabilities,
    VectorChange, VectorFilter, VectorIndexAdmin, VectorQuery, VectorSearchHit, VectorSearchPage,
    VectorSearcher, VectorValue,
};
