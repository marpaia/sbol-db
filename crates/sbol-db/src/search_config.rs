//! Process-level construction of built-in search plugins from JSON config.

#[cfg(feature = "python")]
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use sbol_db_embedding_fastembed::{
    FastEmbedPooling, FastEmbedProvider, FastEmbedProviderConfig, LocalFastEmbedBundleConfig,
};
use sbol_db_search::{
    EmbeddingStrategyConfig, SearchDeployment, SearchDeploymentBuilder, SearchTopologyConfig,
    VectorIndexBindingConfig, VectorIndexMaintenanceConfig,
};
#[cfg(feature = "faiss")]
use sbol_db_search_faiss::{FaissBackendConfig, FaissVectorBackend};
#[cfg(feature = "python")]
use sbol_db_search_python::{load_plugin as load_python_plugin, PythonSearchPluginConfig};
use sbol_db_search_sdk::{DistanceMetric, EmbeddingProvider};
use sbol_db_vector_flat::ExactFlatVectorBackend;
use sbol_db_vector_qdrant::{QdrantRemoteBackend, QdrantRemoteConfig};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchProcessConfig {
    topology: SearchTopologyConfig,
    #[serde(default)]
    embeddings: Vec<EmbeddingPluginConfig>,
    #[serde(default)]
    vector_backends: Vec<VectorBackendPluginConfig>,
    /// Python modules whose `register(search)` function contributes embedding
    /// providers and native strategy instances to this deployment.
    #[cfg(feature = "python")]
    #[serde(default)]
    python_plugins: Vec<PythonSearchPluginConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum EmbeddingPluginConfig {
    FastembedLocal {
        profile: FastEmbedProviderConfig,
        bundle: LocalFastEmbedBundleConfig,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum VectorBackendPluginConfig {
    ExactFlat {
        id: String,
    },
    Qdrant {
        config: QdrantRemoteConfig,
        /// Read the API key from this environment variable. Keeping the key out
        /// of the JSON file is the recommended deployment shape.
        #[serde(default)]
        api_key_env: Option<String>,
    },
    #[cfg(feature = "faiss")]
    Faiss {
        config: FaissBackendConfig,
    },
}

/// Profile identity of the contextual model shipped in the official image.
pub const BUILTIN_BGE_SMALL_PROFILE: &str = "builtin.sbol-text-bge-small.v1";
/// Digest over the five checked-in-manifest artifacts, calculated by
/// `sbol-db util fastembed-revision` before this profile was released.
pub const BUILTIN_BGE_SMALL_REVISION: &str =
    "sha3-256:bf577972c34b37578aa42965fa8401d5538f4b4c007c810332e936548658c7b3";
/// An operator may point a source build at the exact same verified bundle.
pub const BUILTIN_BGE_SMALL_MODEL_DIR_ENV: &str = "SBOL_DB_BGE_SMALL_MODEL_DIR";
const BUILTIN_BGE_SMALL_CONTAINER_DIR: &str = "/opt/sbol-db/models/bge-small-en-v1.5";
const BUILTIN_BGE_SMALL_ONNX_FILE: &str = "model_optimized.onnx";
const BUILTIN_BGE_SMALL_INDEX: &str = "builtin.sbol-text-bge-small.v1";
const BUILTIN_BGE_SMALL_STRATEGY: &str = "builtin.sbol-text-vector.v2";

/// The no-configuration search deployment shipped by the server binary.
///
/// It uses the pinned BGE-small ONNX bundle over the canonical SBOL object text
/// projection and an in-process exact backend. The official image always ships
/// that bundle; a source build must set [`BUILTIN_BGE_SMALL_MODEL_DIR_ENV`] to
/// an artifact fetched with `make model/bge-small`. There is no silent lexical
/// fallback, because serving a different vector space under the same default
/// strategy would make query behavior deployment-dependent.
///
/// The server installs it only when it also embeds a worker, because that
/// worker shares the backend state. Production topologies that separate API
/// and worker processes select a persistent shared backend through
/// [`load_builder`].
pub async fn built_in_text_deployment() -> Result<SearchDeployment> {
    let profile = builtin_bge_small_profile();
    let model_directory = builtin_bge_small_model_dir();
    let bundle = builtin_bge_small_bundle(model_directory.clone());
    let provider =
        tokio::task::spawn_blocking(move || FastEmbedProvider::from_local_bundle(profile, &bundle))
            .await
            .context("joining built-in BGE-small model initialization task")?
            .with_context(|| {
                format!(
                    "loading built-in BGE-small bundle from {}; source builds must run \
                     `make model/bge-small` and set {BUILTIN_BGE_SMALL_MODEL_DIR_ENV}",
                    model_directory.display()
                )
            })?;
    build_builtin_text_deployment(Arc::new(provider))
}

fn builtin_bge_small_profile() -> FastEmbedProviderConfig {
    FastEmbedProviderConfig {
        id: BUILTIN_BGE_SMALL_PROFILE.to_owned(),
        model: "Qdrant/bge-small-en-v1.5-onnx-Q@52398278842ec682c6f32300af41344b1c0b0bb2"
            .to_owned(),
        revision: BUILTIN_BGE_SMALL_REVISION.to_owned(),
        dimension: 384,
        normalization: sbol_db_search_sdk::Normalization::L2,
        // BGE v1.5 supports instruction-free retrieval; the canonical SBOL
        // projection is short and labeled, so documents and queries stay in
        // the model's native text space without an application prompt.
        query_prefix: None,
        document_prefix: None,
        batch_size: 32,
    }
}

fn builtin_bge_small_bundle(directory: PathBuf) -> LocalFastEmbedBundleConfig {
    LocalFastEmbedBundleConfig {
        directory,
        onnx_file: BUILTIN_BGE_SMALL_ONNX_FILE.to_owned(),
        pooling: FastEmbedPooling::Cls,
        max_length: 512,
        intra_threads: Some(2),
    }
}

fn builtin_bge_small_model_dir() -> PathBuf {
    std::env::var_os(BUILTIN_BGE_SMALL_MODEL_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(BUILTIN_BGE_SMALL_CONTAINER_DIR))
}

fn build_builtin_text_deployment(
    embedding: Arc<dyn EmbeddingProvider>,
) -> Result<SearchDeployment> {
    let embedding_profile = embedding.descriptor().id.clone();

    let topology = SearchTopologyConfig {
        default_strategy: BUILTIN_BGE_SMALL_STRATEGY.to_owned(),
        indexes: vec![VectorIndexBindingConfig {
            index: BUILTIN_BGE_SMALL_INDEX.to_owned(),
            backend: "builtin.exact-flat.v1".to_owned(),
            embedding_profile: embedding_profile.clone(),
            vector_name: "content".to_owned(),
            graph_payload_field: "graph".to_owned(),
            maintenance: Some(VectorIndexMaintenanceConfig {
                generation_prefix: "builtin-sbol-text-bge-small".to_owned(),
                distance: DistanceMetric::Cosine,
                batch_size: 128,
                backend_parameters: Default::default(),
            }),
        }],
        embedding_strategies: vec![EmbeddingStrategyConfig {
            id: BUILTIN_BGE_SMALL_STRATEGY.to_owned(),
            version: "2".to_owned(),
            display_name: "Built-in SBOL contextual text vectors".to_owned(),
            description: "BGE-small contextual vectors over canonical SBOL object metadata"
                .to_owned(),
            embedding_profile,
            vector_index: BUILTIN_BGE_SMALL_INDEX.to_owned(),
            vector_name: "content".to_owned(),
            graph_payload_field: "graph".to_owned(),
            distance: DistanceMetric::Cosine,
        }],
    };
    SearchDeploymentBuilder::new(topology)
        .register_embedding(embedding)?
        .register_vector_backend(Arc::new(ExactFlatVectorBackend::new(
            "builtin.exact-flat.v1",
        )))?
        .build()
        .map_err(Into::into)
}

pub async fn load_builder(path: &Path) -> Result<SearchDeploymentBuilder> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("reading search config {}", path.display()))?;
    let config: SearchProcessConfig = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing search config {}", path.display()))?;
    let mut builder = SearchDeploymentBuilder::new(config.topology);

    #[cfg(feature = "python")]
    let mut resolved_embeddings: HashMap<String, Arc<dyn EmbeddingProvider>> = HashMap::new();
    #[cfg(feature = "python")]
    let mut python_strategies = Vec::new();

    #[cfg(feature = "python")]
    for mut plugin_config in config.python_plugins {
        if let Some(plugin_path) = plugin_config.path.as_mut() {
            if plugin_path.is_relative() {
                *plugin_path = path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(&*plugin_path);
            }
        }
        let module = plugin_config.module.clone();
        let plugin = tokio::task::spawn_blocking(move || load_python_plugin(&plugin_config))
            .await
            .with_context(|| format!("joining Python search plugin {module:?} loader"))??;
        for embedding in plugin.embeddings {
            let id = embedding.descriptor().id.clone();
            builder = builder.register_embedding(embedding.clone())?;
            resolved_embeddings.insert(id, embedding);
        }
        for strategy in plugin.embedding_strategies {
            builder = builder.register_embedding_strategy(strategy)?;
        }
        python_strategies.extend(plugin.strategies);
    }

    for embedding in config.embeddings {
        match embedding {
            EmbeddingPluginConfig::FastembedLocal { profile, bundle } => {
                let provider = tokio::task::spawn_blocking(move || {
                    FastEmbedProvider::from_local_bundle(profile, &bundle)
                })
                .await
                .context("joining FastEmbed model initialization task")??;
                let provider: Arc<dyn EmbeddingProvider> = Arc::new(provider);
                #[cfg(feature = "python")]
                resolved_embeddings.insert(provider.descriptor().id.clone(), provider.clone());
                builder = builder.register_embedding(provider)?;
            }
        }
    }

    #[cfg(feature = "python")]
    for strategy in python_strategies {
        let profile = strategy.embedding_profile().to_owned();
        let embedding = resolved_embeddings.get(&profile).cloned().ok_or_else(|| {
            anyhow::anyhow!("Python strategy requires unknown embedding profile {profile:?}")
        })?;
        builder = builder.register_strategy(strategy.bind(embedding)?)?;
    }

    for backend in config.vector_backends {
        match backend {
            VectorBackendPluginConfig::ExactFlat { id } => {
                builder =
                    builder.register_vector_backend(Arc::new(ExactFlatVectorBackend::new(id)))?;
            }
            VectorBackendPluginConfig::Qdrant {
                mut config,
                api_key_env,
            } => {
                if config.api_key.is_some() && api_key_env.is_some() {
                    bail!("Qdrant config cannot set both api_key and api_key_env");
                }
                if let Some(variable) = api_key_env {
                    if variable.trim().is_empty() {
                        bail!("Qdrant api_key_env cannot be empty");
                    }
                    config.api_key = Some(std::env::var(&variable).with_context(|| {
                        format!("reading Qdrant API key environment variable {variable:?}")
                    })?);
                }
                builder =
                    builder.register_vector_backend(Arc::new(QdrantRemoteBackend::new(config)?))?;
            }
            #[cfg(feature = "faiss")]
            VectorBackendPluginConfig::Faiss { config } => {
                builder =
                    builder.register_vector_backend(Arc::new(FaissVectorBackend::open(config)?))?;
            }
        }
    }

    Ok(builder)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use sbol_db_app::AppServices;
    use sbol_db_backend::Backend;
    use sbol_db_search::{
        HashingTextEmbeddingProvider, VectorDocument, VectorRebuildSpec, BUILTIN_SBOL_TEXT_PROFILE,
    };
    use sbol_db_search_sdk::{
        DocumentId, EmbeddingBatch, EmbeddingInput, EmbeddingInputKind, EmbeddingProvider,
        EmbeddingVector, IndexMutationSource, VectorQuery, VectorValue,
    };
    use sbol_db_storage::ListJobsFilter;
    use serde_json::json;

    use super::*;

    fn fixture_builtin_text_deployment() -> SearchDeployment {
        build_builtin_text_deployment(Arc::new(HashingTextEmbeddingProvider::new())).unwrap()
    }

    #[test]
    fn parses_tagged_plugin_configuration() {
        let config: SearchProcessConfig = serde_json::from_value(serde_json::json!({
            "topology": {
                "default_strategy": "semantic.v1",
                "indexes": [{
                    "index": "components",
                    "backend": "flat",
                    "embedding_profile": "local.test.v1",
                    "vector_name": "content",
                    "maintenance": {
                        "generation_prefix": "components-auto",
                        "distance": "cosine"
                    }
                }],
                "embedding_strategies": []
            },
            "embeddings": [{
                "kind": "fastembed_local",
                "profile": {
                    "id": "local.test.v1",
                    "model": "test-model",
                    "revision": "sha3-256:abc",
                    "dimension": 384,
                    "normalization": "l2"
                },
                "bundle": {"directory": "/models/test"}
            }],
            "vector_backends": [{"kind": "exact_flat", "id": "flat"}]
        }))
        .unwrap();

        assert_eq!(config.topology.indexes[0].graph_payload_field, "graph");
        assert!(config.topology.indexes[0].maintenance.is_some());
        assert_eq!(config.embeddings.len(), 1);
        assert_eq!(config.vector_backends.len(), 1);
    }

    #[cfg(feature = "python")]
    #[tokio::test(flavor = "multi_thread")]
    async fn python_plugin_joins_native_deployment_composition() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("composition_fixture.py"),
            r#"
class Embedding:
    def embed(self, texts, *, kind):
        return [[1.0, 0.0] for _ in texts]

class Strategy:
    def search(self, ctx, request):
        return {"items": []}

def register(search):
    search.add_embedding(
        Embedding(),
        id="python.composition.v1",
        model="fixture/model",
        revision="abc123",
        dimension=2,
    )
    search.add_strategy(
        Strategy(),
        id="python.composition-search.v1",
        embedding_profile="python.composition.v1",
        vector_index="python-composition",
    )
"#,
        )
        .unwrap();
        let config_path = directory.path().join("search.json");
        std::fs::write(
            &config_path,
            serde_json::to_vec(&json!({
                "topology": {
                    "default_strategy": "python.composition-search.v1",
                    "indexes": [{
                        "index": "python-composition",
                        "backend": "flat",
                        "embedding_profile": "python.composition.v1",
                        "vector_name": "content"
                    }],
                    "embedding_strategies": []
                },
                "vector_backends": [{"kind": "exact_flat", "id": "flat"}],
                "python_plugins": [{
                    "module": "composition_fixture",
                    "path": "."
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let deployment = load_builder(&config_path).await.unwrap().build().unwrap();
        assert_eq!(
            deployment.runtime().default_strategy(),
            "python.composition-search.v1"
        );
        assert_eq!(
            deployment.maintainers().indexes(),
            vec!["python-composition"]
        );
    }

    #[tokio::test]
    async fn built_in_deployment_has_a_maintained_text_vector_index() {
        let deployment = fixture_builtin_text_deployment();
        assert_eq!(
            deployment.runtime().default_strategy(),
            BUILTIN_BGE_SMALL_STRATEGY
        );
        assert_eq!(
            deployment.maintainers().indexes(),
            vec![BUILTIN_BGE_SMALL_INDEX]
        );
        let maintenance = deployment.maintenance().plugins();
        assert_eq!(maintenance.len(), 1);
        let task = maintenance[0]
            .plan(&sbol_db_search_sdk::IndexMaintenanceEvent::corpus(
                sbol_db_search_sdk::IndexMutationSource::Startup,
            ))
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(task.kind, "rebuild_vector_index");
        assert_eq!(task.payload["artifact_id"], BUILTIN_BGE_SMALL_INDEX);
    }

    #[tokio::test]
    async fn built_in_deployment_queues_a_startup_rebuild() {
        let directory = tempfile::tempdir().unwrap();
        let database_url = format!("sqlite://{}", directory.path().join("sbol.db").display());
        let backend = Backend::open(&database_url).await.unwrap();
        backend
            .migrator
            .as_ref()
            .unwrap()
            .run_migrations()
            .await
            .unwrap();
        let deployment = fixture_builtin_text_deployment();
        let app = AppServices::from_backend(&backend).with_search_deployment(&deployment);

        let receipt = app
            .schedule_search_reconciliation(IndexMutationSource::Startup)
            .await
            .unwrap();
        assert_eq!(receipt.enqueued, 1);
        let jobs = backend
            .jobs
            .list(&ListJobsFilter {
                kind: Some("rebuild_vector_index".to_owned()),
                limit: 10,
                ..ListJobsFilter::default()
            })
            .await
            .unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].payload["artifact_id"], BUILTIN_BGE_SMALL_INDEX);
    }

    #[tokio::test]
    async fn built_in_text_index_fixture_rebuilds_and_returns_a_match() {
        let deployment = fixture_builtin_text_deployment();
        let maintainer = deployment
            .maintainers()
            .get(BUILTIN_BGE_SMALL_INDEX)
            .unwrap();
        maintainer
            .rebuild(
                VectorRebuildSpec {
                    artifact_id: BUILTIN_BGE_SMALL_INDEX.to_owned(),
                    generation: "test".to_owned(),
                    vector_name: "content".to_owned(),
                    embedding_profile: BUILTIN_SBOL_TEXT_PROFILE.to_owned(),
                    distance: DistanceMetric::Cosine,
                    batch_size: 16,
                    backend_parameters: BTreeMap::new(),
                },
                vec![VectorDocument {
                    document_id: DocumentId("https://example.org/pTet".to_owned()),
                    text: "Name: pTet\nRole: promoter\nDescription: tetracycline inducible"
                        .to_owned(),
                    payload: BTreeMap::from([("graph".to_owned(), json!("public"))]),
                }],
            )
            .await
            .unwrap();
        let query = HashingTextEmbeddingProvider::new()
            .embed(EmbeddingBatch {
                profile: BUILTIN_SBOL_TEXT_PROFILE.to_owned(),
                inputs: vec![EmbeddingInput {
                    kind: EmbeddingInputKind::Query,
                    text: "tetracycline promoter".to_owned(),
                }],
            })
            .await
            .unwrap();
        let EmbeddingVector::Dense(vector) = query.vectors.into_iter().next().unwrap() else {
            panic!("built-in provider must return dense vectors");
        };
        let page = maintainer
            .backend()
            .query(VectorQuery {
                index: BUILTIN_BGE_SMALL_INDEX.to_owned(),
                vector_name: "content".to_owned(),
                vector: VectorValue::Dense(vector),
                filter: None,
                limit: 10,
                cursor: None,
                score_threshold: None,
                parameters: BTreeMap::new(),
            })
            .await
            .unwrap();
        assert_eq!(page.items[0].document_id.0, "https://example.org/pTet");
    }

    #[test]
    fn built_in_bge_profile_is_immutable_and_uses_cls_pooling() {
        let profile = builtin_bge_small_profile();
        let bundle = builtin_bge_small_bundle(PathBuf::from("/fixture/model"));
        assert_eq!(profile.id, BUILTIN_BGE_SMALL_PROFILE);
        assert_eq!(profile.revision, BUILTIN_BGE_SMALL_REVISION);
        assert_eq!(profile.dimension, 384);
        assert_eq!(bundle.onnx_file, "model_optimized.onnx");
        assert_eq!(bundle.pooling, FastEmbedPooling::Cls);
    }

    #[cfg(feature = "faiss")]
    #[test]
    fn parses_faiss_backend_configuration() {
        let config: SearchProcessConfig = serde_json::from_value(serde_json::json!({
            "topology": {
                "default_strategy": "legacy.explorer.v1",
                "indexes": [],
                "embedding_strategies": []
            },
            "vector_backends": [{
                "kind": "faiss",
                "config": {
                    "id": "faiss-local",
                    "path": "/var/lib/sbol-db/faiss",
                    "default_nlist": 512,
                    "default_nprobe": 32,
                    "flat_search_cutoff": 1000,
                    "max_query_k": 5000
                }
            }]
        }))
        .unwrap();

        assert_eq!(config.vector_backends.len(), 1);
    }
}
