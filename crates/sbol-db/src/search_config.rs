//! Process-level construction of built-in search plugins from JSON config.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use sbol_db_embedding_fastembed::{
    FastEmbedPooling, FastEmbedProvider, FastEmbedProviderConfig, LocalFastEmbedBundleConfig,
};
use sbol_db_search::{
    EmbeddingStrategyConfig, SearchDeployment, SearchDeploymentBuilder, SearchTopologyConfig,
    VectorIndexBindingConfig, VectorIndexMaintenanceConfig,
};
#[cfg(feature = "faiss")]
use sbol_db_search_faiss::{FaissBackendConfig, FaissVectorBackend};
use sbol_db_search_sdk::{
    DistanceMetric, EmbeddingProvider, IndexMaintenanceDescriptor, IndexMaintenanceEvent,
    IndexMaintenancePlugin, IndexMaintenanceTask, IndexMutationSource, SearchError,
};
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
const BUILTIN_BGE_SMALL_SOURCE_CACHE: &str =
    ".cache/sbol-db/models/bge-small-en-v1.5-onnx-q-5239827";
const BUILTIN_BGE_SMALL_ONNX_FILE: &str = "model_optimized.onnx";
const BUILTIN_BGE_SMALL_INDEX: &str = "builtin.sbol-text-bge-small.v1";
const BUILTIN_BGE_SMALL_STRATEGY: &str = "builtin.sbol-text-vector.v2";
const LEGACY_EXPLORER_MAINTENANCE_ID: &str = "legacy.explorer-index-maintenance.v1";
const LEGACY_EXPLORER_REBUILD_KIND: &str = "rebuild_search_index";

struct LegacyExplorerMaintenance {
    descriptor: IndexMaintenanceDescriptor,
    skip_startup: bool,
}

impl LegacyExplorerMaintenance {
    fn new(skip_startup: bool) -> Self {
        Self {
            descriptor: IndexMaintenanceDescriptor {
                id: LEGACY_EXPLORER_MAINTENANCE_ID.to_owned(),
                display_name: "SBOLExplorer compatibility index maintenance".to_owned(),
                description: "Coalesced rebuilds of the shared ranked text, sequence sketch, cluster, and PageRank indexes".to_owned(),
            },
            skip_startup,
        }
    }
}

#[async_trait]
impl IndexMaintenancePlugin for LegacyExplorerMaintenance {
    fn descriptor(&self) -> &IndexMaintenanceDescriptor {
        &self.descriptor
    }

    async fn plan(
        &self,
        event: &IndexMaintenanceEvent,
    ) -> Result<Vec<IndexMaintenanceTask>, SearchError> {
        if self.skip_startup && event.source == IndexMutationSource::Startup {
            return Ok(Vec::new());
        }
        Ok(vec![IndexMaintenanceTask::new(
            LEGACY_EXPLORER_REBUILD_KIND,
            serde_json::json!({}),
        )])
    }
}

fn legacy_explorer_maintenance(skip_startup: bool) -> Arc<dyn IndexMaintenancePlugin> {
    Arc::new(LegacyExplorerMaintenance::new(skip_startup))
}

/// The no-configuration search deployment shipped by the server binary.
///
/// It uses the pinned BGE-small ONNX bundle over the canonical SBOL object text
/// projection and an in-process exact backend. The official image always ships
/// that bundle. Source builds automatically discover the verified artifact
/// fetched by `make model/bge-small`; [`BUILTIN_BGE_SMALL_MODEL_DIR_ENV`]
/// remains an explicit override. There is no silent lexical fallback, because
/// serving a different vector space under the same default strategy would make
/// query behavior deployment-dependent.
///
/// The server installs it only when it also embeds a worker, because that
/// worker shares the backend state. Production topologies that separate API
/// and worker processes select a persistent shared backend through
/// [`load_builder`].
pub async fn built_in_text_deployment(skip_legacy_startup: bool) -> Result<SearchDeployment> {
    let profile = builtin_bge_small_profile();
    let model_directory = builtin_bge_small_model_dir();
    let bundle = builtin_bge_small_bundle(model_directory.clone());
    let provider =
        tokio::task::spawn_blocking(move || FastEmbedProvider::from_local_bundle(profile, &bundle))
            .await
            .context("joining built-in BGE-small model initialization task")?
            .with_context(|| {
                format!(
                    "loading built-in BGE-small bundle from {}; source builds populate the \
                     auto-discovered cache with `make model/bge-small` (override with \
                     {BUILTIN_BGE_SMALL_MODEL_DIR_ENV})",
                    model_directory.display()
                )
            })?;
    build_builtin_text_deployment(Arc::new(provider), skip_legacy_startup)
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
    resolve_builtin_bge_small_model_dir(
        std::env::var_os(BUILTIN_BGE_SMALL_MODEL_DIR_ENV).map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
        PathBuf::from(BUILTIN_BGE_SMALL_CONTAINER_DIR),
    )
}

fn resolve_builtin_bge_small_model_dir(
    explicit: Option<PathBuf>,
    home: Option<PathBuf>,
    container: PathBuf,
) -> PathBuf {
    if let Some(explicit) = explicit {
        return explicit;
    }

    let source_cache = home.map(|home| home.join(BUILTIN_BGE_SMALL_SOURCE_CACHE));
    if let Some(source_cache) = source_cache.as_ref().filter(|path| path.is_dir()) {
        return source_cache.clone();
    }
    if container.is_dir() {
        return container;
    }

    // Prefer the path populated by the documented Make target in diagnostics,
    // even before it exists. Falling back to /opt on a source checkout made a
    // successful `make model/bge-small` look ineffective.
    source_cache.unwrap_or(container)
}

fn build_builtin_text_deployment(
    embedding: Arc<dyn EmbeddingProvider>,
    skip_legacy_startup: bool,
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
        .register_maintenance_plugin(legacy_explorer_maintenance(skip_legacy_startup))?
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

    for embedding in config.embeddings {
        match embedding {
            EmbeddingPluginConfig::FastembedLocal { profile, bundle } => {
                let provider = tokio::task::spawn_blocking(move || {
                    FastEmbedProvider::from_local_bundle(profile, &bundle)
                })
                .await
                .context("joining FastEmbed model initialization task")??;
                builder = builder.register_embedding(Arc::new(provider))?;
            }
        }
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

    builder
        .register_maintenance_plugin(legacy_explorer_maintenance(false))
        .map_err(Into::into)
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
        build_builtin_text_deployment(Arc::new(HashingTextEmbeddingProvider::new()), false).unwrap()
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
        assert_eq!(maintenance.len(), 2);
        let event = sbol_db_search_sdk::IndexMaintenanceEvent::corpus(
            sbol_db_search_sdk::IndexMutationSource::Startup,
        );
        let mut tasks = Vec::new();
        for plugin in maintenance {
            tasks.extend(plugin.plan(&event).await.unwrap());
        }
        tasks.sort_by(|left, right| left.kind.cmp(&right.kind));
        assert_eq!(
            tasks
                .iter()
                .map(|task| task.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["rebuild_search_index", "rebuild_vector_index"]
        );
        let vector = tasks
            .iter()
            .find(|task| task.kind == "rebuild_vector_index")
            .unwrap();
        assert_eq!(vector.payload["artifact_id"], BUILTIN_BGE_SMALL_INDEX);
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
        assert_eq!(receipt.enqueued, 2);
        let vector_jobs = backend
            .jobs
            .list(&ListJobsFilter {
                kind: Some("rebuild_vector_index".to_owned()),
                limit: 10,
                ..ListJobsFilter::default()
            })
            .await
            .unwrap();
        assert_eq!(vector_jobs.len(), 1);
        assert_eq!(
            vector_jobs[0].payload["artifact_id"],
            BUILTIN_BGE_SMALL_INDEX
        );
        let explorer_jobs = backend
            .jobs
            .list(&ListJobsFilter {
                kind: Some("rebuild_search_index".to_owned()),
                limit: 10,
                ..ListJobsFilter::default()
            })
            .await
            .unwrap();
        assert_eq!(explorer_jobs.len(), 1);
    }

    #[tokio::test]
    async fn populated_durable_text_index_skips_only_legacy_startup_rebuild() {
        let deployment =
            build_builtin_text_deployment(Arc::new(HashingTextEmbeddingProvider::new()), true)
                .unwrap();
        let startup = IndexMaintenanceEvent::corpus(IndexMutationSource::Startup);
        let mutation = IndexMaintenanceEvent::corpus(IndexMutationSource::GraphStore);

        let mut startup_kinds = Vec::new();
        let mut mutation_kinds = Vec::new();
        for plugin in deployment.maintenance().plugins() {
            startup_kinds.extend(
                plugin
                    .plan(&startup)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|task| task.kind),
            );
            mutation_kinds.extend(
                plugin
                    .plan(&mutation)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|task| task.kind),
            );
        }
        startup_kinds.sort();
        mutation_kinds.sort();
        assert_eq!(startup_kinds, vec!["rebuild_vector_index"]);
        assert_eq!(
            mutation_kinds,
            vec!["rebuild_search_index", "rebuild_vector_index"]
        );
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

    #[test]
    fn explicit_builtin_model_directory_always_wins() {
        let explicit = PathBuf::from("/configured/model");
        let selected = resolve_builtin_bge_small_model_dir(
            Some(explicit.clone()),
            Some(PathBuf::from("/home/developer")),
            PathBuf::from("/container/model"),
        );
        assert_eq!(selected, explicit);
    }

    #[test]
    fn source_build_discovers_the_make_target_cache() {
        let home = tempfile::tempdir().unwrap();
        let source_cache = home.path().join(BUILTIN_BGE_SMALL_SOURCE_CACHE);
        std::fs::create_dir_all(&source_cache).unwrap();

        let selected = resolve_builtin_bge_small_model_dir(
            None,
            Some(home.path().to_owned()),
            home.path().join("missing-container-model"),
        );
        assert_eq!(selected, source_cache);
    }

    #[test]
    fn missing_source_bundle_reports_the_path_populated_by_make() {
        let home = tempfile::tempdir().unwrap();
        let selected = resolve_builtin_bge_small_model_dir(
            None,
            Some(home.path().to_owned()),
            home.path().join("missing-container-model"),
        );
        assert_eq!(selected, home.path().join(BUILTIN_BGE_SMALL_SOURCE_CACHE));
    }

    #[test]
    fn packaged_model_is_used_when_the_source_cache_is_absent() {
        let root = tempfile::tempdir().unwrap();
        let container = root.path().join("container-model");
        std::fs::create_dir_all(&container).unwrap();

        let selected = resolve_builtin_bge_small_model_dir(
            None,
            Some(root.path().join("home")),
            container.clone(),
        );
        assert_eq!(selected, container);
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
