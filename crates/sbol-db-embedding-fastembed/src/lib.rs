//! Local dense embeddings through FastEmbed and ONNX Runtime.
//!
//! The provider accepts an already initialized [`fastembed::TextEmbedding`].
//! Model acquisition is deliberately separate: deployments choose whether to
//! load pinned local bytes, use a controlled cache, or enable FastEmbed's
//! online model support. The sbol-db profile always carries the immutable
//! revision that was actually loaded.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sbol_db_search_sdk::{
    DataEgress, EmbeddingBatch, EmbeddingDescriptor, EmbeddingInputKind, EmbeddingOutput,
    EmbeddingProvider, EmbeddingVector, Normalization, SearchError,
};
use serde::{Deserialize, Serialize};

pub use fastembed;

const DEFAULT_BATCH_SIZE: usize = 64;

/// Reproducible profile metadata plus text-role prefixes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FastEmbedProviderConfig {
    pub id: String,
    /// Model repository/code or another stable operator-facing name.
    pub model: String,
    /// Immutable weight commit or content digest. A floating branch such as
    /// `main` is rejected because it cannot identify an index artifact.
    pub revision: String,
    pub dimension: usize,
    #[serde(default = "default_normalization")]
    pub normalization: Normalization,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_prefix: Option<String>,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

const fn default_batch_size() -> usize {
    DEFAULT_BATCH_SIZE
}

const fn default_normalization() -> Normalization {
    Normalization::None
}

trait DenseEngine: Send {
    fn embed(&mut self, texts: &[String], batch_size: usize) -> Result<Vec<Vec<f32>>, String>;
}

impl DenseEngine for fastembed::TextEmbedding {
    fn embed(&mut self, texts: &[String], batch_size: usize) -> Result<Vec<Vec<f32>>, String> {
        fastembed::TextEmbedding::embed(self, texts, Some(batch_size))
            .map_err(|error| error.to_string())
    }
}

/// Thread-safe async adapter around FastEmbed's mutable synchronous session.
/// CPU inference runs on Tokio's blocking pool rather than an async executor
/// worker. The mutex serializes a single ONNX session; deployments that need
/// more inference concurrency register multiple provider instances.
pub struct FastEmbedProvider {
    descriptor: EmbeddingDescriptor,
    config: FastEmbedProviderConfig,
    engine: Arc<Mutex<Box<dyn DenseEngine>>>,
}

impl FastEmbedProvider {
    pub fn new(
        config: FastEmbedProviderConfig,
        model: fastembed::TextEmbedding,
    ) -> Result<Self, SearchError> {
        Self::with_engine(config, Box::new(model))
    }

    fn with_engine(
        config: FastEmbedProviderConfig,
        engine: Box<dyn DenseEngine>,
    ) -> Result<Self, SearchError> {
        validate_config(&config)?;
        let descriptor = EmbeddingDescriptor {
            id: config.id.clone(),
            provider: "fastembed".to_owned(),
            model: config.model.clone(),
            revision: config.revision.clone(),
            dimension: config.dimension,
            normalization: config.normalization,
            data_egress: DataEgress::None,
        };
        Ok(Self {
            descriptor,
            config,
            engine: Arc::new(Mutex::new(engine)),
        })
    }
}

#[async_trait]
impl EmbeddingProvider for FastEmbedProvider {
    fn descriptor(&self) -> &EmbeddingDescriptor {
        &self.descriptor
    }

    async fn embed(&self, batch: EmbeddingBatch) -> Result<EmbeddingOutput, SearchError> {
        if batch.profile != self.descriptor.id {
            return Err(SearchError::Configuration(format!(
                "embedding batch requests profile {:?}, but provider exposes {:?}",
                batch.profile, self.descriptor.id
            )));
        }
        if batch.inputs.is_empty() {
            return Ok(EmbeddingOutput {
                vectors: Vec::new(),
            });
        }

        let texts = batch
            .inputs
            .into_iter()
            .map(|input| {
                let prefix = match input.kind {
                    EmbeddingInputKind::Query => self.config.query_prefix.as_deref(),
                    EmbeddingInputKind::Document => self.config.document_prefix.as_deref(),
                };
                match prefix {
                    Some(prefix) => format!("{prefix}{}", input.text),
                    None => input.text,
                }
            })
            .collect::<Vec<_>>();
        let engine = Arc::clone(&self.engine);
        let batch_size = self.config.batch_size;
        let input_count = texts.len();
        let expected_dimension = self.descriptor.dimension;
        let normalization = self.descriptor.normalization;
        let profile = self.descriptor.id.clone();
        let vectors = tokio::task::spawn_blocking(move || {
            let mut engine = engine.lock().map_err(|_| {
                SearchError::Backend(format!(
                    "FastEmbed profile {profile:?} session lock is poisoned"
                ))
            })?;
            engine
                .embed(&texts, batch_size)
                .map_err(|error| SearchError::Backend(format!("FastEmbed inference: {error}")))
        })
        .await
        .map_err(|error| SearchError::Backend(format!("FastEmbed task failed: {error}")))??;

        if vectors.len() != input_count {
            return Err(SearchError::Backend(format!(
                "FastEmbed profile {:?} returned {} vectors for {} inputs",
                self.descriptor.id,
                vectors.len(),
                input_count
            )));
        }
        let vectors = vectors
            .into_iter()
            .map(|vector| {
                validate_vector(vector, expected_dimension, normalization)
                    .map(EmbeddingVector::Dense)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(EmbeddingOutput { vectors })
    }
}

fn validate_config(config: &FastEmbedProviderConfig) -> Result<(), SearchError> {
    if config.id.trim().is_empty() || config.model.trim().is_empty() {
        return Err(SearchError::Configuration(
            "FastEmbed profile id and model cannot be empty".to_owned(),
        ));
    }
    let revision = config.revision.trim();
    if revision.is_empty()
        || revision.eq_ignore_ascii_case("main")
        || revision.eq_ignore_ascii_case("latest")
    {
        return Err(SearchError::Configuration(
            "FastEmbed revision must be an immutable commit or content digest".to_owned(),
        ));
    }
    if config.dimension == 0 || config.batch_size == 0 {
        return Err(SearchError::Configuration(
            "FastEmbed dimension and batch_size must be greater than zero".to_owned(),
        ));
    }
    Ok(())
}

fn validate_vector(
    mut vector: Vec<f32>,
    dimension: usize,
    normalization: Normalization,
) -> Result<Vec<f32>, SearchError> {
    if vector.len() != dimension {
        return Err(SearchError::Backend(format!(
            "FastEmbed returned dimension {}, expected {dimension}",
            vector.len()
        )));
    }
    if vector.iter().any(|value| !value.is_finite()) {
        return Err(SearchError::Backend(
            "FastEmbed returned a non-finite value".to_owned(),
        ));
    }
    if normalization == Normalization::L2 {
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        if norm == 0.0 {
            return Err(SearchError::Backend(
                "FastEmbed returned a zero vector for an L2-normalized profile".to_owned(),
            ));
        }
        for value in &mut vector {
            *value /= norm;
        }
    }
    Ok(vector)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use sbol_db_search_sdk::{EmbeddingInput, EmbeddingInputKind};

    use super::*;

    struct StubEngine {
        observed: Arc<StdMutex<Vec<Vec<String>>>>,
        output: Vec<Vec<f32>>,
    }

    impl DenseEngine for StubEngine {
        fn embed(&mut self, texts: &[String], _batch_size: usize) -> Result<Vec<Vec<f32>>, String> {
            self.observed.lock().unwrap().push(texts.to_vec());
            Ok(self.output.clone())
        }
    }

    fn config() -> FastEmbedProviderConfig {
        FastEmbedProviderConfig {
            id: "local.minilm.v1".to_owned(),
            model: "Qdrant/all-MiniLM-L6-v2-onnx".to_owned(),
            revision: "sha256:weights".to_owned(),
            dimension: 2,
            normalization: Normalization::L2,
            query_prefix: Some("query: ".to_owned()),
            document_prefix: Some("passage: ".to_owned()),
            batch_size: 8,
        }
    }

    #[tokio::test]
    async fn applies_role_prefixes_and_guarantees_normalization() {
        let observed = Arc::new(StdMutex::new(Vec::new()));
        let provider = FastEmbedProvider::with_engine(
            config(),
            Box::new(StubEngine {
                observed: Arc::clone(&observed),
                output: vec![vec![3.0, 4.0], vec![0.0, 2.0]],
            }),
        )
        .unwrap();
        let output = provider
            .embed(EmbeddingBatch {
                profile: "local.minilm.v1".to_owned(),
                inputs: vec![
                    EmbeddingInput {
                        kind: EmbeddingInputKind::Query,
                        text: "promoter".to_owned(),
                    },
                    EmbeddingInput {
                        kind: EmbeddingInputKind::Document,
                        text: "component".to_owned(),
                    },
                ],
            })
            .await
            .unwrap();

        assert_eq!(
            observed.lock().unwrap()[0],
            vec!["query: promoter", "passage: component"]
        );
        assert_eq!(
            output.vectors,
            vec![
                EmbeddingVector::Dense(vec![0.6, 0.8]),
                EmbeddingVector::Dense(vec![0.0, 1.0]),
            ]
        );
    }

    #[test]
    fn rejects_floating_revisions_and_bad_dimensions() {
        let engine = || {
            Box::new(StubEngine {
                observed: Arc::new(StdMutex::new(Vec::new())),
                output: Vec::new(),
            }) as Box<dyn DenseEngine>
        };
        let mut invalid = config();
        invalid.revision = "main".to_owned();
        assert!(matches!(
            FastEmbedProvider::with_engine(invalid, engine()),
            Err(SearchError::Configuration(_))
        ));

        let mut invalid = config();
        invalid.dimension = 0;
        assert!(matches!(
            FastEmbedProvider::with_engine(invalid, engine()),
            Err(SearchError::Configuration(_))
        ));
    }

    #[tokio::test]
    async fn rejects_profile_mismatch_before_inference() {
        let observed = Arc::new(StdMutex::new(Vec::new()));
        let provider = FastEmbedProvider::with_engine(
            config(),
            Box::new(StubEngine {
                observed: Arc::clone(&observed),
                output: vec![vec![1.0, 0.0]],
            }),
        )
        .unwrap();
        let result = provider
            .embed(EmbeddingBatch {
                profile: "other".to_owned(),
                inputs: vec![EmbeddingInput {
                    kind: EmbeddingInputKind::Query,
                    text: "promoter".to_owned(),
                }],
            })
            .await;
        assert!(matches!(result, Err(SearchError::Configuration(_))));
        assert!(observed.lock().unwrap().is_empty());
    }
}
