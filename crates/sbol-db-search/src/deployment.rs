//! Typed startup assembly for search plugins.
//!
//! Concrete embedding and vector crates construct their implementations and
//! register them here. The topology references those implementations by stable
//! IDs, validates cross-plugin requirements once, and yields the runtime,
//! scoped vector router, and matching maintenance coordinators as one unit.

use std::collections::HashMap;
use std::sync::Arc;

use sbol_db_search_sdk::{
    EmbeddingProvider, FilterCapability, SearchError, SearchStrategy, StrategyRegistry,
    VectorBackend,
};
use serde::{Deserialize, Serialize};

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
}

fn default_graph_payload_field() -> String {
    "graph".to_owned()
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
}

/// Fluent Rust assembly API for application binaries and embedders.
pub struct SearchDeploymentBuilder {
    config: SearchTopologyConfig,
    embeddings: HashMap<String, Arc<dyn EmbeddingProvider>>,
    backends: HashMap<String, Arc<dyn VectorBackend>>,
    strategies: Vec<Arc<dyn SearchStrategy>>,
}

impl SearchDeploymentBuilder {
    pub fn new(config: SearchTopologyConfig) -> Self {
        Self {
            config,
            embeddings: HashMap::new(),
            backends: HashMap::new(),
            strategies: Vec::new(),
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

    pub fn build(self) -> Result<SearchDeployment, SearchError> {
        let SearchTopologyConfig {
            default_strategy,
            indexes,
            embedding_strategies,
        } = self.config;
        let mut router = VectorRouter::new();
        let mut maintainers = HashMap::new();
        let mut bindings = HashMap::new();

        for binding in &indexes {
            validate_binding(binding)?;
            if bindings.insert(binding.index.clone(), binding).is_some() {
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
            maintainers.insert(
                binding.index.clone(),
                Arc::new(VectorIndexMaintainer::new(
                    embedding.clone(),
                    backend.clone(),
                )),
            );
        }

        let mut strategy_builder = StrategyRegistry::builder();
        for strategy in self.strategies {
            validate_strategy_requirements(strategy.as_ref(), &self.embeddings, &bindings)?;
            strategy_builder = strategy_builder
                .register_arc(strategy)
                .map_err(|error| configuration(error.to_string()))?;
        }
        for strategy_config in embedding_strategies {
            let binding = bindings.get(&strategy_config.vector_index).ok_or_else(|| {
                configuration(format!(
                    "embedding strategy {:?} references unknown logical index {:?}",
                    strategy_config.id, strategy_config.vector_index
                ))
            })?;
            if binding.embedding_profile != strategy_config.embedding_profile {
                return Err(configuration(format!(
                    "embedding strategy {:?} uses profile {:?}, but index {:?} is maintained with {:?}",
                    strategy_config.id,
                    strategy_config.embedding_profile,
                    binding.index,
                    binding.embedding_profile
                )));
            }
            if binding.vector_name != strategy_config.vector_name {
                return Err(configuration(format!(
                    "embedding strategy {:?} queries vector {:?}, but index {:?} is configured with {:?}",
                    strategy_config.id,
                    strategy_config.vector_name,
                    binding.index,
                    binding.vector_name
                )));
            }
            if binding.graph_payload_field != strategy_config.graph_payload_field {
                return Err(configuration(format!(
                    "embedding strategy {:?} and index {:?} disagree on graph payload field",
                    strategy_config.id, binding.index
                )));
            }
            let backend = self
                .backends
                .get(&binding.backend)
                .expect("validated binding backend must remain registered");
            if !backend
                .descriptor()
                .capabilities
                .distances
                .contains(&strategy_config.distance)
            {
                return Err(configuration(format!(
                    "embedding strategy {:?} requests {:?}, but backend {:?} does not support it",
                    strategy_config.id, strategy_config.distance, binding.backend
                )));
            }
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
            maintainers: Arc::new(VectorIndexMaintainerRegistry {
                entries: maintainers,
            }),
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

fn validate_strategy_requirements(
    strategy: &dyn SearchStrategy,
    embeddings: &HashMap<String, Arc<dyn EmbeddingProvider>>,
    bindings: &HashMap<String, &VectorIndexBindingConfig>,
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
}
