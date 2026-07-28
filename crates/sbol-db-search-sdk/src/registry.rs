use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    EmbeddingProvider, RegistrationError, SearchStrategy, StrategyDescriptor, VectorBackend,
    VectorBackendDescriptor,
};

/// Immutable lookup of configured strategies.
pub struct StrategyRegistry {
    entries: HashMap<String, Arc<dyn SearchStrategy>>,
}

impl StrategyRegistry {
    pub fn builder() -> StrategyRegistryBuilder {
        StrategyRegistryBuilder::default()
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn SearchStrategy>> {
        self.entries.get(id).cloned()
    }

    pub fn descriptors(&self) -> Vec<StrategyDescriptor> {
        let mut descriptors: Vec<_> = self
            .entries
            .values()
            .map(|entry| entry.descriptor().clone())
            .collect();
        descriptors.sort_by(|a, b| a.id.cmp(&b.id));
        descriptors
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Default)]
pub struct StrategyRegistryBuilder {
    entries: HashMap<String, Arc<dyn SearchStrategy>>,
}

impl StrategyRegistryBuilder {
    pub fn register<S>(mut self, strategy: S) -> Result<Self, RegistrationError>
    where
        S: SearchStrategy,
    {
        self.insert(Arc::new(strategy))?;
        Ok(self)
    }

    pub fn register_arc(
        mut self,
        strategy: Arc<dyn SearchStrategy>,
    ) -> Result<Self, RegistrationError> {
        self.insert(strategy)?;
        Ok(self)
    }

    fn insert(&mut self, strategy: Arc<dyn SearchStrategy>) -> Result<(), RegistrationError> {
        let id = strategy.descriptor().id.trim();
        if id.is_empty() {
            return Err(RegistrationError::EmptyId { kind: "strategy" });
        }
        if self.entries.contains_key(id) {
            return Err(RegistrationError::Duplicate {
                kind: "strategy",
                id: id.to_owned(),
            });
        }
        self.entries.insert(id.to_owned(), strategy);
        Ok(())
    }

    pub fn build(self) -> StrategyRegistry {
        StrategyRegistry {
            entries: self.entries,
        }
    }
}

/// Immutable lookup of configured embedding profiles.
pub struct EmbeddingRegistry {
    entries: HashMap<String, Arc<dyn EmbeddingProvider>>,
}

impl EmbeddingRegistry {
    pub fn builder() -> EmbeddingRegistryBuilder {
        EmbeddingRegistryBuilder::default()
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn EmbeddingProvider>> {
        self.entries.get(id).cloned()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Default)]
pub struct EmbeddingRegistryBuilder {
    entries: HashMap<String, Arc<dyn EmbeddingProvider>>,
}

impl EmbeddingRegistryBuilder {
    pub fn register<E>(mut self, provider: E) -> Result<Self, RegistrationError>
    where
        E: EmbeddingProvider,
    {
        self.insert(Arc::new(provider))?;
        Ok(self)
    }

    pub fn register_arc(
        mut self,
        provider: Arc<dyn EmbeddingProvider>,
    ) -> Result<Self, RegistrationError> {
        self.insert(provider)?;
        Ok(self)
    }

    fn insert(&mut self, provider: Arc<dyn EmbeddingProvider>) -> Result<(), RegistrationError> {
        let id = provider.descriptor().id.trim();
        if id.is_empty() {
            return Err(RegistrationError::EmptyId { kind: "embedding" });
        }
        if self.entries.contains_key(id) {
            return Err(RegistrationError::Duplicate {
                kind: "embedding",
                id: id.to_owned(),
            });
        }
        self.entries.insert(id.to_owned(), provider);
        Ok(())
    }

    pub fn build(self) -> EmbeddingRegistry {
        EmbeddingRegistry {
            entries: self.entries,
        }
    }
}

/// Immutable lookup of configured vector backend instances.
pub struct VectorBackendRegistry {
    entries: HashMap<String, Arc<dyn VectorBackend>>,
}

impl VectorBackendRegistry {
    pub fn builder() -> VectorBackendRegistryBuilder {
        VectorBackendRegistryBuilder::default()
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn VectorBackend>> {
        self.entries.get(id).cloned()
    }

    pub fn descriptors(&self) -> Vec<VectorBackendDescriptor> {
        let mut descriptors: Vec<_> = self
            .entries
            .values()
            .map(|entry| entry.descriptor().clone())
            .collect();
        descriptors.sort_by(|a, b| a.id.cmp(&b.id));
        descriptors
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Default)]
pub struct VectorBackendRegistryBuilder {
    entries: HashMap<String, Arc<dyn VectorBackend>>,
}

impl VectorBackendRegistryBuilder {
    pub fn register<V>(mut self, backend: V) -> Result<Self, RegistrationError>
    where
        V: VectorBackend,
    {
        self.insert(Arc::new(backend))?;
        Ok(self)
    }

    pub fn register_arc(
        mut self,
        backend: Arc<dyn VectorBackend>,
    ) -> Result<Self, RegistrationError> {
        self.insert(backend)?;
        Ok(self)
    }

    fn insert(&mut self, backend: Arc<dyn VectorBackend>) -> Result<(), RegistrationError> {
        let id = backend.descriptor().id.trim();
        if id.is_empty() {
            return Err(RegistrationError::EmptyId {
                kind: "vector backend",
            });
        }
        if self.entries.contains_key(id) {
            return Err(RegistrationError::Duplicate {
                kind: "vector backend",
                id: id.to_owned(),
            });
        }
        self.entries.insert(id.to_owned(), backend);
        Ok(())
    }

    pub fn build(self) -> VectorBackendRegistry {
        VectorBackendRegistry {
            entries: self.entries,
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use crate::{
        DataEgress, FilterCapability, PaginationCapability, SearchContext, SearchError, SearchPage,
        SearchRequest, StrategyCapabilities, StrategyRequirements, TotalCapability,
    };

    struct StubStrategy {
        descriptor: StrategyDescriptor,
    }

    impl StubStrategy {
        fn new(id: &str) -> Self {
            Self {
                descriptor: StrategyDescriptor {
                    id: id.to_owned(),
                    version: "1".to_owned(),
                    display_name: id.to_owned(),
                    description: String::new(),
                    capabilities: StrategyCapabilities {
                        inputs: Vec::new(),
                        filters: Vec::new(),
                        filter_execution: FilterCapability::None,
                        pagination: PaginationCapability::FirstPageOnly,
                        totals: TotalCapability::Unknown,
                        deterministic: true,
                        explanations: false,
                        data_egress: DataEgress::None,
                    },
                    requirements: StrategyRequirements::default(),
                },
            }
        }
    }

    #[async_trait]
    impl SearchStrategy for StubStrategy {
        fn descriptor(&self) -> &StrategyDescriptor {
            &self.descriptor
        }

        async fn search(
            &self,
            _ctx: SearchContext,
            _request: SearchRequest,
        ) -> Result<SearchPage, SearchError> {
            Err(SearchError::Unsupported("stub".to_owned()))
        }
    }

    #[test]
    fn strategy_registry_is_sorted_and_rejects_duplicates() {
        let builder = StrategyRegistry::builder()
            .register(StubStrategy::new("zeta"))
            .expect("first registration")
            .register(StubStrategy::new("alpha"))
            .expect("second registration");
        let err = match builder.register(StubStrategy::new("alpha")) {
            Ok(_) => panic!("duplicate should fail"),
            Err(err) => err,
        };
        assert_eq!(
            err,
            RegistrationError::Duplicate {
                kind: "strategy",
                id: "alpha".to_owned(),
            }
        );

        let registry = StrategyRegistry::builder()
            .register(StubStrategy::new("zeta"))
            .expect("zeta")
            .register(StubStrategy::new("alpha"))
            .expect("alpha")
            .build();
        let ids: Vec<_> = registry
            .descriptors()
            .into_iter()
            .map(|descriptor| descriptor.id)
            .collect();
        assert_eq!(ids, ["alpha", "zeta"]);
        assert!(registry.get("alpha").is_some());
    }
}
