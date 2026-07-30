//! Typed startup assembly for search plugins.
//!
//! Concrete embedding and vector crates construct their implementations and
//! register them here. The topology references those implementations by stable
//! IDs, validates cross-plugin requirements once, and yields the runtime,
//! scoped vector router, and matching maintenance coordinators as one unit.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use sbol_db_search_sdk::{
    DistanceMetric, EmbeddingProvider, FilterCapability, IndexMaintenanceDescriptor,
    IndexMaintenanceEvent, IndexMaintenancePlugin, IndexMaintenanceRegistry,
    IndexMaintenanceRegistryBuilder, IndexMaintenanceTask, IndexMutationScope, SearchError,
    SearchStrategy, StrategyRegistry, VectorBackend,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    EmbeddingSearchStrategy, EmbeddingStrategyConfig, SearchRuntime, VectorIndexMaintainer,
    VectorRouter,
};

/// One logical index binding shared by query routing and maintenance jobs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VectorIndexBindingConfig {
    pub index: String,
    pub backend: String,
    pub embedding_profile: String,
    pub vector_name: String,
    #[serde(default = "default_graph_payload_field")]
    pub graph_payload_field: String,
    /// Optional automatic maintenance policy. An omitted policy preserves the
    /// prior manual-only lifecycle for this logical index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintenance: Option<VectorIndexMaintenanceConfig>,
}

fn default_graph_payload_field() -> String {
    "graph".to_owned()
}

/// How the built-in vector plugin turns committed application writes into
/// durable vector maintenance work.
///
/// The generation name is minted per rebuild so repeated corpus-level writes
/// cannot collide with a ready generation. Document-precise writes use the
/// active generation at execution time when the backend supports incremental
/// updates; otherwise they fall back to a complete rebuild.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VectorIndexMaintenanceConfig {
    pub generation_prefix: String,
    pub distance: DistanceMetric,
    #[serde(default = "default_maintenance_batch_size")]
    pub batch_size: usize,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub backend_parameters: std::collections::BTreeMap<String, serde_json::Value>,
}

const fn default_maintenance_batch_size() -> usize {
    64
}

/// Serializable plugin topology. Concrete provider/backend secrets and model
/// loading details remain in their own configuration types.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchTopologyConfig {
    pub default_strategy: String,
    #[serde(default)]
    pub indexes: Vec<VectorIndexBindingConfig>,
    #[serde(default)]
    pub embedding_strategies: Vec<EmbeddingStrategyConfig>,
}

/// Immutable mapping used by durable jobs to select the same provider/backend
/// pair that serves a logical index.
#[derive(Clone, Default)]
pub struct VectorIndexMaintainerRegistry {
    entries: HashMap<String, Arc<VectorIndexMaintainer>>,
}

impl VectorIndexMaintainerRegistry {
    pub fn get(&self, index: &str) -> Option<Arc<VectorIndexMaintainer>> {
        self.entries.get(index).cloned()
    }

    pub fn indexes(&self) -> Vec<String> {
        let mut indexes = self.entries.keys().cloned().collect::<Vec<_>>();
        indexes.sort();
        indexes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Fully validated, shareable search runtime assembled at process startup.
pub struct SearchDeployment {
    runtime: Arc<SearchRuntime>,
    router: Arc<VectorRouter>,
    maintainers: Arc<VectorIndexMaintainerRegistry>,
    maintenance: Arc<IndexMaintenanceRegistry>,
}

impl SearchDeployment {
    pub fn runtime(&self) -> Arc<SearchRuntime> {
        self.runtime.clone()
    }

    pub fn router(&self) -> Arc<VectorRouter> {
        self.router.clone()
    }

    pub fn maintainers(&self) -> Arc<VectorIndexMaintainerRegistry> {
        self.maintainers.clone()
    }

    /// Plugin-defined policies that turn committed application writes into
    /// durable index-maintenance tasks.
    pub fn maintenance(&self) -> Arc<IndexMaintenanceRegistry> {
        self.maintenance.clone()
    }
}

/// Fluent Rust assembly API for application binaries and embedders.
pub struct SearchDeploymentBuilder {
    config: SearchTopologyConfig,
    embeddings: HashMap<String, Arc<dyn EmbeddingProvider>>,
    backends: HashMap<String, Arc<dyn VectorBackend>>,
    strategies: Vec<Arc<dyn SearchStrategy>>,
    maintenance_plugins: Vec<Arc<dyn IndexMaintenancePlugin>>,
}

struct VectorPlane {
    router: VectorRouter,
    maintainers: Arc<VectorIndexMaintainerRegistry>,
    maintenance: Arc<IndexMaintenanceRegistry>,
    bindings: HashMap<String, VectorIndexBindingConfig>,
}

impl SearchDeploymentBuilder {
    pub fn new(config: SearchTopologyConfig) -> Self {
        Self {
            config,
            embeddings: HashMap::new(),
            backends: HashMap::new(),
            strategies: Vec::new(),
            maintenance_plugins: Vec::new(),
        }
    }

    pub fn register_embedding(
        mut self,
        provider: Arc<dyn EmbeddingProvider>,
    ) -> Result<Self, SearchError> {
        let id = provider.descriptor().id.trim().to_owned();
        if id.is_empty() {
            return Err(configuration("embedding profile id cannot be empty"));
        }
        if self.embeddings.insert(id.clone(), provider).is_some() {
            return Err(configuration(format!("duplicate embedding profile {id:?}")));
        }
        Ok(self)
    }

    pub fn register_vector_backend(
        mut self,
        backend: Arc<dyn VectorBackend>,
    ) -> Result<Self, SearchError> {
        let id = backend.descriptor().id.trim().to_owned();
        if id.is_empty() {
            return Err(configuration("vector backend id cannot be empty"));
        }
        if self.backends.insert(id.clone(), backend).is_some() {
            return Err(configuration(format!("duplicate vector backend {id:?}")));
        }
        Ok(self)
    }

    /// Add a classic, neural, hybrid, or agentic strategy implemented through
    /// the public SDK. Its declared embedding/index requirements are checked
    /// against the assembled topology during `build`.
    pub fn register_strategy(
        mut self,
        strategy: Arc<dyn SearchStrategy>,
    ) -> Result<Self, SearchError> {
        let id = strategy.descriptor().id.trim().to_owned();
        if id.is_empty() {
            return Err(configuration("strategy id cannot be empty"));
        }
        if self
            .strategies
            .iter()
            .any(|registered| registered.descriptor().id == id)
        {
            return Err(configuration(format!("duplicate strategy {id:?}")));
        }
        self.strategies.push(strategy);
        Ok(self)
    }

    /// Add a configured instance of the native dense embedding strategy.
    /// Runtime language bridges use this after they have collected plugin
    /// registrations; the provider itself is still resolved by stable profile
    /// ID during `build`.
    pub fn register_embedding_strategy(
        mut self,
        strategy: EmbeddingStrategyConfig,
    ) -> Result<Self, SearchError> {
        let id = strategy.id.trim();
        if id.is_empty() {
            return Err(configuration("strategy id cannot be empty"));
        }
        if self
            .config
            .embedding_strategies
            .iter()
            .any(|registered| registered.id == id)
            || self
                .strategies
                .iter()
                .any(|registered| registered.descriptor().id == id)
        {
            return Err(configuration(format!("duplicate strategy {id:?}")));
        }
        self.config.embedding_strategies.push(strategy);
        Ok(self)
    }

    /// Register a storage-neutral maintenance plugin. The application later
    /// gives its resulting tasks to the durable job queue after a successful
    /// data mutation.
    pub fn register_maintenance_plugin(
        mut self,
        plugin: Arc<dyn IndexMaintenancePlugin>,
    ) -> Result<Self, SearchError> {
        let id = plugin.descriptor().id.trim().to_owned();
        if id.is_empty() {
            return Err(configuration("index maintenance plugin id cannot be empty"));
        }
        if self
            .maintenance_plugins
            .iter()
            .any(|registered| registered.descriptor().id == id)
        {
            return Err(configuration(format!(
                "duplicate index maintenance plugin {id:?}"
            )));
        }
        self.maintenance_plugins.push(plugin);
        Ok(self)
    }

    pub fn build(self) -> Result<SearchDeployment, SearchError> {
        let VectorPlane {
            router,
            maintainers,
            maintenance,
            bindings,
        } = self.assemble_vector_plane()?;
        let SearchTopologyConfig {
            default_strategy,
            indexes: _,
            embedding_strategies,
        } = self.config;

        let mut strategy_builder = StrategyRegistry::builder();
        for strategy in self.strategies {
            validate_strategy_requirements(strategy.as_ref(), &self.embeddings, &bindings)?;
            strategy_builder = strategy_builder
                .register_arc(strategy)
                .map_err(|error| configuration(error.to_string()))?;
        }
        for strategy_config in embedding_strategies {
            let embedding = self
                .embeddings
                .get(&strategy_config.embedding_profile)
                .expect("validated binding embedding must remain registered")
                .clone();
            let strategy = EmbeddingSearchStrategy::new(strategy_config, embedding)?;
            strategy_builder = strategy_builder
                .register(strategy)
                .map_err(|error| configuration(error.to_string()))?;
        }

        let runtime = SearchRuntime::new(strategy_builder.build(), default_strategy)?;
        Ok(SearchDeployment {
            runtime: Arc::new(runtime),
            router: Arc::new(router),
            maintainers,
            maintenance,
        })
    }

    /// Assemble only index maintenance. Standalone workers use this path when
    /// they do not own query-time strategy dependencies such as the in-process
    /// legacy text index.
    pub fn build_maintenance(self) -> Result<Arc<VectorIndexMaintainerRegistry>, SearchError> {
        Ok(self.assemble_vector_plane()?.maintainers)
    }

    fn assemble_vector_plane(&self) -> Result<VectorPlane, SearchError> {
        let mut router = VectorRouter::new();
        let mut maintainers = HashMap::new();
        let mut maintenance = IndexMaintenanceRegistryBuilder::default();
        let mut bindings = HashMap::new();

        for binding in &self.config.indexes {
            validate_binding(binding)?;
            if bindings
                .insert(binding.index.clone(), binding.clone())
                .is_some()
            {
                return Err(configuration(format!(
                    "duplicate logical vector index {:?}",
                    binding.index
                )));
            }
            let backend = self.backends.get(&binding.backend).ok_or_else(|| {
                configuration(format!(
                    "logical index {:?} references unknown vector backend {:?}",
                    binding.index, binding.backend
                ))
            })?;
            let embedding = self
                .embeddings
                .get(&binding.embedding_profile)
                .ok_or_else(|| {
                    configuration(format!(
                        "logical index {:?} references unknown embedding profile {:?}",
                        binding.index, binding.embedding_profile
                    ))
                })?;
            if !backend.descriptor().capabilities.dense {
                return Err(configuration(format!(
                    "logical index {:?} requires dense vectors, but backend {:?} does not support them",
                    binding.index, binding.backend
                )));
            }
            if backend.descriptor().capabilities.filter_execution != FilterCapability::Native {
                return Err(configuration(format!(
                    "logical index {:?} requires native graph filtering from backend {:?}",
                    binding.index, binding.backend
                )));
            }
            router = router.register(
                binding.index.clone(),
                backend.clone(),
                binding.graph_payload_field.clone(),
            )?;
            let maintainer = Arc::new(VectorIndexMaintainer::new(
                embedding.clone(),
                backend.clone(),
            ));
            if let Some(policy) = binding.maintenance.clone() {
                validate_maintenance_policy(binding, &policy, backend.as_ref())?;
                maintenance = maintenance
                    .register_arc(Arc::new(VectorIndexMaintenancePlugin::new(
                        binding,
                        policy,
                        maintainer.clone(),
                    )))
                    .map_err(|error| configuration(error.to_string()))?;
            }
            maintainers.insert(binding.index.clone(), maintainer);
        }

        for plugin in &self.maintenance_plugins {
            maintenance = maintenance
                .register_arc(plugin.clone())
                .map_err(|error| configuration(error.to_string()))?;
        }

        for strategy in &self.config.embedding_strategies {
            let binding = bindings.get(&strategy.vector_index).ok_or_else(|| {
                configuration(format!(
                    "embedding strategy {:?} references unknown logical index {:?}",
                    strategy.id, strategy.vector_index
                ))
            })?;
            validate_embedding_strategy_binding(strategy, binding, &self.backends)?;
        }

        Ok(VectorPlane {
            router,
            maintainers: Arc::new(VectorIndexMaintainerRegistry {
                entries: maintainers,
            }),
            maintenance: Arc::new(maintenance.build()),
            bindings,
        })
    }
}

fn validate_binding(binding: &VectorIndexBindingConfig) -> Result<(), SearchError> {
    for (name, value) in [
        ("index", &binding.index),
        ("backend", &binding.backend),
        ("embedding_profile", &binding.embedding_profile),
        ("vector_name", &binding.vector_name),
        ("graph_payload_field", &binding.graph_payload_field),
    ] {
        if value.trim().is_empty() {
            return Err(configuration(format!(
                "vector index binding {name} cannot be empty"
            )));
        }
    }
    Ok(())
}

fn validate_maintenance_policy(
    binding: &VectorIndexBindingConfig,
    policy: &VectorIndexMaintenanceConfig,
    backend: &dyn VectorBackend,
) -> Result<(), SearchError> {
    if policy.generation_prefix.trim().is_empty() {
        return Err(configuration(format!(
            "vector index {:?} maintenance generation_prefix cannot be empty",
            binding.index
        )));
    }
    if policy.batch_size == 0 {
        return Err(configuration(format!(
            "vector index {:?} maintenance batch_size must be greater than zero",
            binding.index
        )));
    }
    if !backend
        .descriptor()
        .capabilities
        .distances
        .contains(&policy.distance)
    {
        return Err(configuration(format!(
            "vector index {:?} maintenance distance {:?} is not supported by backend {:?}",
            binding.index,
            policy.distance,
            backend.descriptor().id,
        )));
    }
    Ok(())
}

/// Built-in maintenance policy for one configured vector index. Its durable
/// task kinds are implemented by `sbol-db-jobs`; the SDK contract deliberately
/// keeps the application and plugin layers free of that dependency.
struct VectorIndexMaintenancePlugin {
    descriptor: IndexMaintenanceDescriptor,
    index: String,
    vector_name: String,
    embedding_profile: String,
    policy: VectorIndexMaintenanceConfig,
    maintainer: Arc<VectorIndexMaintainer>,
}

impl VectorIndexMaintenancePlugin {
    fn new(
        binding: &VectorIndexBindingConfig,
        policy: VectorIndexMaintenanceConfig,
        maintainer: Arc<VectorIndexMaintainer>,
    ) -> Self {
        Self {
            descriptor: IndexMaintenanceDescriptor {
                id: format!("vector.{}.maintenance.v1", binding.index),
                display_name: format!("{} vector index maintenance", binding.index),
                description: "Maintains an active vector generation after committed SBOL writes"
                    .to_owned(),
            },
            index: binding.index.clone(),
            vector_name: binding.vector_name.clone(),
            embedding_profile: binding.embedding_profile.clone(),
            policy,
            maintainer,
        }
    }

    fn rebuild_task(&self) -> IndexMaintenanceTask {
        let generation = format!(
            "{}-{}",
            self.policy.generation_prefix,
            Uuid::new_v4().simple()
        );
        IndexMaintenanceTask::new(
            "rebuild_vector_index",
            json!({
                "artifact_id": self.index,
                "generation": generation,
                "vector_name": self.vector_name,
                "embedding_profile": self.embedding_profile,
                "distance": self.policy.distance,
                "batch_size": self.policy.batch_size,
                "backend_parameters": self.policy.backend_parameters,
            }),
        )
    }
}

#[async_trait]
impl IndexMaintenancePlugin for VectorIndexMaintenancePlugin {
    fn descriptor(&self) -> &IndexMaintenanceDescriptor {
        &self.descriptor
    }

    async fn plan(
        &self,
        event: &IndexMaintenanceEvent,
    ) -> Result<Vec<IndexMaintenanceTask>, SearchError> {
        let IndexMutationScope::Documents { document_ids } = &event.scope else {
            return Ok(vec![self.rebuild_task()]);
        };
        if document_ids.is_empty() {
            return Ok(Vec::new());
        }
        let capabilities = &self.maintainer.backend().descriptor().capabilities;
        let active = self
            .maintainer
            .active_generation(&self.index)
            .await?
            .is_some();
        if !active || !capabilities.incremental_updates || !capabilities.deletes {
            return Ok(vec![self.rebuild_task()]);
        }

        Ok(vec![IndexMaintenanceTask::new(
            "maintain_vector_index",
            json!({
                "artifact_id": self.index,
                "document_ids": document_ids,
                "batch_size": self.policy.batch_size,
            }),
        )])
    }
}

fn validate_embedding_strategy_binding(
    strategy: &EmbeddingStrategyConfig,
    binding: &VectorIndexBindingConfig,
    backends: &HashMap<String, Arc<dyn VectorBackend>>,
) -> Result<(), SearchError> {
    if binding.embedding_profile != strategy.embedding_profile {
        return Err(configuration(format!(
            "embedding strategy {:?} uses profile {:?}, but index {:?} is maintained with {:?}",
            strategy.id, strategy.embedding_profile, binding.index, binding.embedding_profile
        )));
    }
    if binding.vector_name != strategy.vector_name {
        return Err(configuration(format!(
            "embedding strategy {:?} queries vector {:?}, but index {:?} is configured with {:?}",
            strategy.id, strategy.vector_name, binding.index, binding.vector_name
        )));
    }
    if binding.graph_payload_field != strategy.graph_payload_field {
        return Err(configuration(format!(
            "embedding strategy {:?} and index {:?} disagree on graph payload field",
            strategy.id, binding.index
        )));
    }
    if let Some(policy) = &binding.maintenance {
        if policy.distance != strategy.distance {
            return Err(configuration(format!(
                "embedding strategy {:?} uses {:?}, but automatic maintenance for index {:?} rebuilds with {:?}",
                strategy.id, strategy.distance, binding.index, policy.distance
            )));
        }
    }
    let backend = backends
        .get(&binding.backend)
        .expect("validated binding backend must remain registered");
    if !backend
        .descriptor()
        .capabilities
        .distances
        .contains(&strategy.distance)
    {
        return Err(configuration(format!(
            "embedding strategy {:?} requests {:?}, but backend {:?} does not support it",
            strategy.id, strategy.distance, binding.backend
        )));
    }
    Ok(())
}

fn validate_strategy_requirements(
    strategy: &dyn SearchStrategy,
    embeddings: &HashMap<String, Arc<dyn EmbeddingProvider>>,
    bindings: &HashMap<String, VectorIndexBindingConfig>,
) -> Result<(), SearchError> {
    let descriptor = strategy.descriptor();
    for profile in &descriptor.requirements.embedding_profiles {
        if !embeddings.contains_key(profile) {
            return Err(configuration(format!(
                "strategy {:?} requires unknown embedding profile {profile:?}",
                descriptor.id
            )));
        }
    }
    for index in &descriptor.requirements.vector_indexes {
        if !bindings.contains_key(index) {
            return Err(configuration(format!(
                "strategy {:?} requires unknown vector index {index:?}",
                descriptor.id
            )));
        }
    }
    Ok(())
}

fn configuration(message: impl Into<String>) -> SearchError {
    SearchError::Configuration(message.into())
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use sbol_db_search_sdk::{
        DataEgress, DistanceMetric, EmbeddingBatch, EmbeddingDescriptor, EmbeddingOutput,
        EmbeddingVector, Normalization,
    };
    use sbol_db_vector_flat::ExactFlatVectorBackend;

    use super::*;

    struct StubEmbedding {
        descriptor: EmbeddingDescriptor,
    }

    #[async_trait]
    impl EmbeddingProvider for StubEmbedding {
        fn descriptor(&self) -> &EmbeddingDescriptor {
            &self.descriptor
        }

        async fn embed(&self, batch: EmbeddingBatch) -> Result<EmbeddingOutput, SearchError> {
            Ok(EmbeddingOutput {
                vectors: batch
                    .inputs
                    .iter()
                    .map(|_| EmbeddingVector::Dense(vec![1.0, 0.0]))
                    .collect(),
            })
        }
    }

    fn provider() -> Arc<dyn EmbeddingProvider> {
        Arc::new(StubEmbedding {
            descriptor: EmbeddingDescriptor {
                id: "local.test.v1".to_owned(),
                provider: "test".to_owned(),
                model: "fixture".to_owned(),
                revision: "sha256:abc".to_owned(),
                dimension: 2,
                normalization: Normalization::L2,
                data_egress: DataEgress::None,
            },
        })
    }

    fn topology() -> SearchTopologyConfig {
        SearchTopologyConfig {
            default_strategy: "semantic.v1".to_owned(),
            indexes: vec![VectorIndexBindingConfig {
                index: "components".to_owned(),
                backend: "flat".to_owned(),
                embedding_profile: "local.test.v1".to_owned(),
                vector_name: "content".to_owned(),
                graph_payload_field: "graph".to_owned(),
                maintenance: None,
            }],
            embedding_strategies: vec![EmbeddingStrategyConfig {
                id: "semantic.v1".to_owned(),
                version: "1".to_owned(),
                display_name: "Semantic".to_owned(),
                description: "test strategy".to_owned(),
                embedding_profile: "local.test.v1".to_owned(),
                vector_index: "components".to_owned(),
                vector_name: "content".to_owned(),
                graph_payload_field: "graph".to_owned(),
                distance: DistanceMetric::Cosine,
            }],
        }
    }

    #[test]
    fn assembles_query_and_maintenance_from_one_topology() {
        let deployment = SearchDeploymentBuilder::new(topology())
            .register_embedding(provider())
            .unwrap()
            .register_vector_backend(Arc::new(ExactFlatVectorBackend::new("flat")))
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(deployment.runtime().default_strategy(), "semantic.v1");
        assert_eq!(deployment.runtime().descriptors().len(), 1);
        assert_eq!(deployment.maintainers().indexes(), vec!["components"]);
    }

    #[test]
    fn maintenance_only_build_does_not_require_query_strategy_dependencies() {
        let mut topology = topology();
        topology.default_strategy = "legacy.explorer.v1".to_owned();
        let maintainers = SearchDeploymentBuilder::new(topology)
            .register_embedding(provider())
            .unwrap()
            .register_vector_backend(Arc::new(ExactFlatVectorBackend::new("flat")))
            .unwrap()
            .build_maintenance()
            .unwrap();

        assert_eq!(maintainers.indexes(), vec!["components"]);
    }

    #[test]
    fn rejects_strategy_and_maintenance_profile_drift() {
        let mut topology = topology();
        topology.embedding_strategies[0].embedding_profile = "other".to_owned();
        let result = SearchDeploymentBuilder::new(topology)
            .register_embedding(provider())
            .unwrap()
            .register_vector_backend(Arc::new(ExactFlatVectorBackend::new("flat")))
            .unwrap()
            .build();
        let error = match result {
            Ok(_) => panic!("profile drift must fail startup assembly"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("maintained with"));
    }

    #[test]
    fn rejects_strategy_and_maintenance_distance_drift() {
        let mut topology = topology();
        topology.indexes[0].maintenance = Some(VectorIndexMaintenanceConfig {
            generation_prefix: "components-auto".to_owned(),
            distance: DistanceMetric::Dot,
            batch_size: 64,
            backend_parameters: Default::default(),
        });
        let result = SearchDeploymentBuilder::new(topology)
            .register_embedding(provider())
            .unwrap()
            .register_vector_backend(Arc::new(ExactFlatVectorBackend::new("flat")))
            .unwrap()
            .build();
        let error = match result {
            Ok(_) => panic!("distance drift must fail startup assembly"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("automatic maintenance"));
    }

    #[tokio::test]
    async fn vector_maintenance_bootstraps_then_updates_the_active_generation() {
        let mut topology = topology();
        topology.indexes[0].maintenance = Some(VectorIndexMaintenanceConfig {
            generation_prefix: "components-auto".to_owned(),
            distance: DistanceMetric::Cosine,
            batch_size: 17,
            backend_parameters: Default::default(),
        });
        let deployment = SearchDeploymentBuilder::new(topology)
            .register_embedding(provider())
            .unwrap()
            .register_vector_backend(Arc::new(ExactFlatVectorBackend::new("flat")))
            .unwrap()
            .build()
            .unwrap();
        let plugin = deployment
            .maintenance()
            .plugins()
            .pop()
            .expect("configured vector policy registers a maintenance plugin");
        let event = IndexMaintenanceEvent::documents(
            sbol_db_search_sdk::IndexMutationSource::Submission,
            [sbol_db_search_sdk::DocumentId(
                "https://example.org/component".to_owned(),
            )],
        );

        let bootstrap = plugin.plan(&event).await.unwrap();
        assert_eq!(bootstrap.len(), 1);
        assert_eq!(bootstrap[0].kind, "rebuild_vector_index");
        assert_eq!(bootstrap[0].payload["batch_size"], 17);

        deployment
            .maintainers()
            .get("components")
            .unwrap()
            .rebuild(
                crate::VectorRebuildSpec {
                    artifact_id: "components".to_owned(),
                    generation: "g1".to_owned(),
                    vector_name: "content".to_owned(),
                    embedding_profile: "local.test.v1".to_owned(),
                    distance: DistanceMetric::Cosine,
                    batch_size: 17,
                    backend_parameters: Default::default(),
                },
                Vec::<crate::VectorDocument>::new(),
            )
            .await
            .unwrap();

        let incremental = plugin.plan(&event).await.unwrap();
        assert_eq!(incremental.len(), 1);
        assert_eq!(incremental[0].kind, "maintain_vector_index");
        assert_eq!(
            incremental[0].payload["document_ids"],
            serde_json::json!(["https://example.org/component"])
        );

        let corpus = plugin
            .plan(&IndexMaintenanceEvent::corpus(
                sbol_db_search_sdk::IndexMutationSource::SparqlUpdate,
            ))
            .await
            .unwrap();
        assert_eq!(corpus[0].kind, "rebuild_vector_index");
    }
}
