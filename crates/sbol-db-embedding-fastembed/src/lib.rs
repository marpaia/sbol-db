//! Local dense embeddings through FastEmbed and ONNX Runtime.
//!
//! The provider accepts an already initialized [`fastembed::TextEmbedding`].
//! Model acquisition is deliberately separate: deployments choose whether to
//! load pinned local bytes, use a controlled cache, or enable FastEmbed's
//! online model support. The sbol-db profile always carries the immutable
//! revision that was actually loaded.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sbol_db_search_sdk::{
    DataEgress, EmbeddingBatch, EmbeddingDescriptor, EmbeddingInputKind, EmbeddingOutput,
    EmbeddingProvider, EmbeddingVector, Normalization, SearchError,
};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};

pub use fastembed;

const DEFAULT_BATCH_SIZE: usize = 64;

/// Pooling applied when loading a bring-your-own ONNX model bundle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FastEmbedPooling {
    #[default]
    Cls,
    Mean,
}

/// Files and inference limits for an immutable local FastEmbed model bundle.
/// Tokenizer filenames follow the Hugging Face convention used by FastEmbed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalFastEmbedBundleConfig {
    pub directory: PathBuf,
    #[serde(default = "default_onnx_file")]
    pub onnx_file: String,
    #[serde(default)]
    pub pooling: FastEmbedPooling,
    #[serde(default = "default_max_length")]
    pub max_length: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intra_threads: Option<usize>,
}

fn default_onnx_file() -> String {
    "model.onnx".to_owned()
}

const fn default_max_length() -> usize {
    512
}

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

    /// Load a local ONNX/tokenizer bundle and verify that its content digest is
    /// exactly the immutable revision declared by the profile.
    pub fn from_local_bundle(
        config: FastEmbedProviderConfig,
        bundle: &LocalFastEmbedBundleConfig,
    ) -> Result<Self, SearchError> {
        validate_config(&config)?;
        validate_bundle_config(bundle)?;
        ensure_dynamic_ort_path()?;
        let files = LocalBundleFiles::read(bundle)?;
        let revision = files.revision();
        if config.revision != revision {
            return Err(SearchError::Configuration(format!(
                "FastEmbed profile {:?} declares revision {:?}, but local bundle hashes to {:?}",
                config.id, config.revision, revision
            )));
        }

        let model = fastembed::UserDefinedEmbeddingModel::new(
            files.onnx,
            fastembed::TokenizerFiles {
                tokenizer_file: files.tokenizer,
                config_file: files.config,
                special_tokens_map_file: files.special_tokens_map,
                tokenizer_config_file: files.tokenizer_config,
            },
        )
        .with_pooling(match bundle.pooling {
            FastEmbedPooling::Cls => fastembed::Pooling::Cls,
            FastEmbedPooling::Mean => fastembed::Pooling::Mean,
        });
        let mut options =
            fastembed::InitOptionsUserDefined::new().with_max_length(bundle.max_length);
        if let Some(threads) = bundle.intra_threads {
            options = options.with_intra_threads(threads);
        }
        let model = fastembed::TextEmbedding::try_new_from_user_defined(model, options).map_err(
            |error| SearchError::Configuration(format!("loading local FastEmbed bundle: {error}")),
        )?;
        Self::new(config, model)
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

// `ort-load-dynamic` otherwise defers this error until ONNX Runtime's global
// initialization. Some versions of the upstream loader re-enter that global
// initializer while formatting a load failure, leaving the process blocked
// when ORT_DYLIB_PATH is absent. Fail before constructing FastEmbed so source
// deployments receive a direct, actionable configuration error.
#[cfg(feature = "dynamic-ort")]
fn ensure_dynamic_ort_path() -> Result<(), SearchError> {
    let path = std::env::var_os("ORT_DYLIB_PATH").ok_or_else(|| {
        SearchError::Configuration(
            "local FastEmbed requires ORT_DYLIB_PATH to name a compatible ONNX Runtime library"
                .to_owned(),
        )
    })?;
    let path = PathBuf::from(path);
    if !path.is_file() {
        return Err(SearchError::Configuration(format!(
            "ORT_DYLIB_PATH does not name a readable ONNX Runtime library: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(feature = "dynamic-ort"))]
fn ensure_dynamic_ort_path() -> Result<(), SearchError> {
    Ok(())
}

/// Calculate the revision operators place in [`FastEmbedProviderConfig`]
/// before loading the ONNX session.
pub fn local_bundle_revision(bundle: &LocalFastEmbedBundleConfig) -> Result<String, SearchError> {
    validate_bundle_config(bundle)?;
    Ok(LocalBundleFiles::read(bundle)?.revision())
}

struct LocalBundleFiles {
    onnx: Vec<u8>,
    tokenizer: Vec<u8>,
    config: Vec<u8>,
    special_tokens_map: Vec<u8>,
    tokenizer_config: Vec<u8>,
}

impl LocalBundleFiles {
    fn read(bundle: &LocalFastEmbedBundleConfig) -> Result<Self, SearchError> {
        Ok(Self {
            onnx: read_model_file(&bundle.directory, &bundle.onnx_file)?,
            tokenizer: read_model_file(&bundle.directory, "tokenizer.json")?,
            config: read_model_file(&bundle.directory, "config.json")?,
            special_tokens_map: read_model_file(&bundle.directory, "special_tokens_map.json")?,
            tokenizer_config: read_model_file(&bundle.directory, "tokenizer_config.json")?,
        })
    }

    fn revision(&self) -> String {
        let mut hasher = Sha3_256::new();
        hash_artifact(&mut hasher, "onnx", &self.onnx);
        hash_artifact(&mut hasher, "tokenizer.json", &self.tokenizer);
        hash_artifact(&mut hasher, "config.json", &self.config);
        hash_artifact(
            &mut hasher,
            "special_tokens_map.json",
            &self.special_tokens_map,
        );
        hash_artifact(&mut hasher, "tokenizer_config.json", &self.tokenizer_config);
        format!("sha3-256:{}", hex::encode(hasher.finalize()))
    }
}

fn hash_artifact(hasher: &mut Sha3_256, label: &str, bytes: &[u8]) {
    hasher.update((label.len() as u64).to_be_bytes());
    hasher.update(label.as_bytes());
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn read_model_file(directory: &Path, name: &str) -> Result<Vec<u8>, SearchError> {
    std::fs::read(directory.join(name)).map_err(|error| {
        SearchError::Configuration(format!(
            "reading FastEmbed model file {:?}: {error}",
            directory.join(name)
        ))
    })
}

fn validate_bundle_config(bundle: &LocalFastEmbedBundleConfig) -> Result<(), SearchError> {
    if bundle.onnx_file.trim().is_empty() || bundle.max_length == 0 {
        return Err(SearchError::Configuration(
            "FastEmbed local bundle onnx_file and max_length must be non-empty/non-zero".to_owned(),
        ));
    }
    if bundle.intra_threads == Some(0) {
        return Err(SearchError::Configuration(
            "FastEmbed local bundle intra_threads must be greater than zero".to_owned(),
        ));
    }
    Ok(())
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

    #[test]
    fn local_bundle_revision_covers_every_required_artifact() {
        let directory = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            ("weights.onnx", b"onnx".as_slice()),
            ("tokenizer.json", b"tokenizer".as_slice()),
            ("config.json", b"config".as_slice()),
            ("special_tokens_map.json", b"special".as_slice()),
            ("tokenizer_config.json", b"tokenizer-config".as_slice()),
        ] {
            std::fs::write(directory.path().join(name), bytes).unwrap();
        }
        let bundle = LocalFastEmbedBundleConfig {
            directory: directory.path().to_path_buf(),
            onnx_file: "weights.onnx".to_owned(),
            pooling: FastEmbedPooling::Mean,
            max_length: 512,
            intra_threads: Some(2),
        };

        let first = local_bundle_revision(&bundle).unwrap();
        assert!(first.starts_with("sha3-256:"));
        std::fs::write(directory.path().join("tokenizer.json"), b"changed").unwrap();
        let second = local_bundle_revision(&bundle).unwrap();
        assert_ne!(first, second);
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
