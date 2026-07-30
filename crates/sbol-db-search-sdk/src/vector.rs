use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{DocumentId, VectorError};

/// Distance functions understood by the backend-neutral vector contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistanceMetric {
    Cosine,
    Dot,
    Euclidean,
    Manhattan,
    Hamming,
    Jaccard,
}

/// Sparse vector with parallel index and value arrays.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SparseVector {
    pub indices: Vec<u32>,
    pub values: Vec<f32>,
}

/// Vector representations accepted by backend plugins.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum VectorValue {
    Dense(Vec<f32>),
    Sparse(SparseVector),
    MultiDense(Vec<Vec<f32>>),
}

/// Portable filter algebra. Backend adapters must reject expressions they
/// cannot execute natively rather than silently post-filtering them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum VectorFilter {
    Match {
        field: String,
        value: Value,
    },
    Any {
        field: String,
        values: Vec<Value>,
    },
    Range {
        field: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gte: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lte: Option<f64>,
    },
    And {
        clauses: Vec<VectorFilter>,
    },
    Or {
        clauses: Vec<VectorFilter>,
    },
    Not {
        clause: Box<VectorFilter>,
    },
}

/// A strategy-facing vector request. The runtime supplies a scoped vector
/// facade that conjoins the caller's authorization filter before the backend
/// receives this value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorQuery {
    pub index: String,
    pub vector_name: String,
    pub vector: VectorValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<VectorFilter>,
    pub limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_threshold: Option<f32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorSearchHit {
    pub document_id: DocumentId,
    pub score: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorSearchPage {
    pub items: Vec<VectorSearchHit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// The only vector interface exposed directly to strategies. It is bound to an
/// authorization scope by the application runtime.
#[async_trait]
pub trait ScopedVectorSearch: Send + Sync + 'static {
    async fn query(&self, query: VectorQuery) -> Result<VectorSearchPage, VectorError>;
}

/// Capabilities used to validate strategy requirements at startup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VectorCapabilities {
    pub dense: bool,
    pub sparse: bool,
    pub multi_dense: bool,
    pub distances: Vec<DistanceMetric>,
    pub filter_execution: crate::FilterCapability,
    pub persistent: bool,
    pub incremental_updates: bool,
    pub deletes: bool,
    pub atomic_activation: bool,
    pub snapshots: bool,
}

/// Identity and placement information for one configured vector backend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VectorBackendDescriptor {
    pub id: String,
    pub kind: String,
    pub remote: bool,
    pub capabilities: VectorCapabilities,
}

#[async_trait]
pub trait VectorSearcher: Send + Sync + 'static {
    fn descriptor(&self) -> &VectorBackendDescriptor;
    async fn query(&self, query: VectorQuery) -> Result<VectorSearchPage, VectorError>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IndexGenerationSpec {
    pub artifact_id: String,
    pub generation: String,
    pub vector_name: String,
    pub dimension: usize,
    pub distance: DistanceMetric,
    /// Exact embedding implementation used to construct this generation.
    /// Older or externally managed generations may omit it, but the
    /// high-level incremental maintainer will refuse to mix new vectors into
    /// a generation whose embedding space it cannot verify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<EmbeddingProvenance>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, Value>,
}

/// Stable identity of the embedding space behind one vector generation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingProvenance {
    pub profile: String,
    pub provider: String,
    pub model: String,
    pub revision: String,
    pub normalization: crate::Normalization,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationHandle {
    pub artifact_id: String,
    pub generation: String,
    pub locator: String,
}

/// Observable state for one index generation. Activation is the only operation
/// that changes which generation serves queries for an artifact. A backend may
/// also accept changes to the active generation when it advertises
/// [`VectorCapabilities::incremental_updates`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GenerationStatus {
    pub handle: GenerationHandle,
    pub spec: IndexGenerationSpec,
    pub active: bool,
    pub vector_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum VectorChange {
    Upsert {
        document_id: DocumentId,
        vectors: BTreeMap<String, VectorValue>,
        payload: BTreeMap<String, Value>,
    },
    Delete {
        document_id: DocumentId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyReceipt {
    /// Number of accepted operations. Idempotent no-op deletes count as
    /// accepted so an at-least-once caller can verify a complete batch.
    pub applied: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRef {
    pub locator: String,
}

#[async_trait]
pub trait VectorIndexAdmin: Send + Sync + 'static {
    async fn create_generation(
        &self,
        spec: IndexGenerationSpec,
    ) -> Result<GenerationHandle, VectorError>;

    /// Apply an ordered batch to a generation. Implementations validate the
    /// complete batch before mutation. Applying to an active generation is
    /// permitted only when `incremental_updates` is advertised; an idempotent
    /// delete of an absent document still contributes to `ApplyReceipt`.
    async fn apply(
        &self,
        generation: &GenerationHandle,
        changes: Vec<VectorChange>,
    ) -> Result<ApplyReceipt, VectorError>;

    async fn flush(&self, generation: &GenerationHandle) -> Result<(), VectorError>;
    async fn optimize(&self, generation: &GenerationHandle) -> Result<(), VectorError>;
    async fn snapshot(&self, generation: &GenerationHandle) -> Result<SnapshotRef, VectorError>;

    /// Atomically make this generation serve queries addressed to its
    /// `artifact_id`. The prior generation remains intact until explicitly
    /// deleted, which makes rollback an activation rather than a rebuild.
    async fn activate(&self, generation: &GenerationHandle) -> Result<(), VectorError>;

    /// Enumerate generations so maintenance jobs can reconcile desired and
    /// actual state after a restart or partial failure.
    async fn generations(&self, artifact_id: &str) -> Result<Vec<GenerationStatus>, VectorError>;

    /// Retire one inactive generation. Backends must reject deletion of the
    /// active generation instead of implicitly changing query behavior.
    async fn delete_generation(&self, generation: &GenerationHandle) -> Result<(), VectorError>;
}

/// Complete backend plugin used by the runtime and index maintenance jobs.
pub trait VectorBackend: VectorSearcher + VectorIndexAdmin {}

impl<T> VectorBackend for T where T: VectorSearcher + VectorIndexAdmin {}
