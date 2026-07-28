use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::SearchError;

/// Whether an embedding is produced for a query or a corpus document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingInputKind {
    Query,
    Document,
}

/// Normalization promised by an embedding profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Normalization {
    None,
    L2,
}

/// Identity and reproducibility metadata for an embedding provider profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingDescriptor {
    pub id: String,
    pub provider: String,
    pub model: String,
    pub revision: String,
    pub dimension: usize,
    pub normalization: Normalization,
    pub data_egress: crate::DataEgress,
}

/// One string to embed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingInput {
    pub kind: EmbeddingInputKind,
    pub text: String,
}

/// A homogeneous embedding request for one configured profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingBatch {
    pub profile: String,
    pub inputs: Vec<EmbeddingInput>,
}

/// Dense and sparse forms supported by embedding plugins.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum EmbeddingVector {
    Dense(Vec<f32>),
    Sparse(crate::SparseVector),
    MultiDense(Vec<Vec<f32>>),
}

/// Ordered output corresponding one-for-one with the input batch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingOutput {
    pub vectors: Vec<EmbeddingVector>,
}

/// A pluggable local or remote embedding implementation.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync + 'static {
    fn descriptor(&self) -> &EmbeddingDescriptor;

    async fn embed(&self, batch: EmbeddingBatch) -> Result<EmbeddingOutput, SearchError>;
}
