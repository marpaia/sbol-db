//! Durable maintenance contracts for search-index plugins.
//!
//! This module intentionally stops at a storage-neutral job description. The
//! application owns the durable job queue and invokes registered plugins after
//! a successful write; a plugin decides whether that write warrants a narrow
//! document synchronization or a full corpus rebuild.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{DocumentId, RegistrationError, SearchError};

/// The application surface through which a committed data mutation arrived.
///
/// Plugins may use this to choose a more conservative maintenance path for
/// opaque graph or SPARQL writes than for a typed object submission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexMutationSource {
    Submission,
    CollectionSync,
    ObjectMutation,
    ObjectEdit,
    Attachment,
    SparqlUpdate,
    GraphStore,
    RestImport,
    RestGraphDelete,
    BackgroundImport,
    Startup,
}

/// The precision with which a committed write identifies its affected corpus.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum IndexMutationScope {
    /// The write identified every search document whose desired vector state
    /// may have changed. IDs are sorted and deduplicated by
    /// [`IndexMaintenanceEvent::documents`].
    Documents { document_ids: Vec<DocumentId> },
    /// The write has corpus-level or opaque effects. A plugin should plan a
    /// reconciliation-capable operation rather than assume that a narrow
    /// document update is complete.
    Corpus,
}

/// A committed application write that search indexes may need to maintain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexMaintenanceEvent {
    pub source: IndexMutationSource,
    #[serde(flatten)]
    pub scope: IndexMutationScope,
}

impl IndexMaintenanceEvent {
    /// Construct a document-precise event with deterministic, unique IDs.
    pub fn documents(
        source: IndexMutationSource,
        document_ids: impl IntoIterator<Item = DocumentId>,
    ) -> Self {
        let mut document_ids = document_ids.into_iter().collect::<Vec<_>>();
        document_ids.sort();
        document_ids.dedup();
        Self {
            source,
            scope: IndexMutationScope::Documents { document_ids },
        }
    }

    /// Construct an event that requires corpus-level reconciliation.
    pub const fn corpus(source: IndexMutationSource) -> Self {
        Self {
            source,
            scope: IndexMutationScope::Corpus,
        }
    }
}

/// Stable metadata for one maintenance plugin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexMaintenanceDescriptor {
    pub id: String,
    pub display_name: String,
    pub description: String,
}

/// A storage-neutral durable-job request emitted by a maintenance plugin.
///
/// The application turns this into its own queue's job type. `None` for every
/// optional scheduling field asks the application queue to use its defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IndexMaintenanceTask {
    pub kind: String,
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

impl IndexMaintenanceTask {
    /// Make a job request that uses the queue's default scheduling policy.
    pub fn new(kind: impl Into<String>, payload: Value) -> Self {
        Self {
            kind: kind.into(),
            payload,
            queue: None,
            priority: None,
            max_attempts: None,
            idempotency_key: None,
        }
    }
}

/// A search plugin's policy for keeping one or more artifacts current.
///
/// Implementations must treat a repeated event as safe: the application may
/// submit the same desired-state intent more than once after a write retry.
#[async_trait]
pub trait IndexMaintenancePlugin: Send + Sync {
    fn descriptor(&self) -> &IndexMaintenanceDescriptor;

    /// Produce the durable work required after `event` committed.
    async fn plan(
        &self,
        event: &IndexMaintenanceEvent,
    ) -> Result<Vec<IndexMaintenanceTask>, SearchError>;
}

/// Immutable collection of maintenance plugins assembled at process startup.
#[derive(Default)]
pub struct IndexMaintenanceRegistry {
    entries: HashMap<String, Arc<dyn IndexMaintenancePlugin>>,
}

impl IndexMaintenanceRegistry {
    pub fn builder() -> IndexMaintenanceRegistryBuilder {
        IndexMaintenanceRegistryBuilder::default()
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn IndexMaintenancePlugin>> {
        self.entries.get(id).cloned()
    }

    pub fn descriptors(&self) -> Vec<IndexMaintenanceDescriptor> {
        let mut descriptors = self
            .entries
            .values()
            .map(|entry| entry.descriptor().clone())
            .collect::<Vec<_>>();
        descriptors.sort_by(|left, right| left.id.cmp(&right.id));
        descriptors
    }

    pub fn plugins(&self) -> Vec<Arc<dyn IndexMaintenancePlugin>> {
        let mut plugins = self.entries.values().cloned().collect::<Vec<_>>();
        plugins.sort_by(|left, right| left.descriptor().id.cmp(&right.descriptor().id));
        plugins
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Startup-time builder for [`IndexMaintenanceRegistry`].
#[derive(Default)]
pub struct IndexMaintenanceRegistryBuilder {
    entries: HashMap<String, Arc<dyn IndexMaintenancePlugin>>,
}

impl IndexMaintenanceRegistryBuilder {
    pub fn register<P>(mut self, plugin: P) -> Result<Self, RegistrationError>
    where
        P: IndexMaintenancePlugin + 'static,
    {
        self.insert(Arc::new(plugin))?;
        Ok(self)
    }

    pub fn register_arc(
        mut self,
        plugin: Arc<dyn IndexMaintenancePlugin>,
    ) -> Result<Self, RegistrationError> {
        self.insert(plugin)?;
        Ok(self)
    }

    fn insert(&mut self, plugin: Arc<dyn IndexMaintenancePlugin>) -> Result<(), RegistrationError> {
        let id = plugin.descriptor().id.trim();
        if id.is_empty() {
            return Err(RegistrationError::EmptyId {
                kind: "index maintenance plugin",
            });
        }
        if self.entries.contains_key(id) {
            return Err(RegistrationError::Duplicate {
                kind: "index maintenance plugin",
                id: id.to_owned(),
            });
        }
        self.entries.insert(id.to_owned(), plugin);
        Ok(())
    }

    pub fn build(self) -> IndexMaintenanceRegistry {
        IndexMaintenanceRegistry {
            entries: self.entries,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_events_are_deterministic() {
        let event = IndexMaintenanceEvent::documents(
            IndexMutationSource::Submission,
            [
                DocumentId("https://example.org/b".to_owned()),
                DocumentId("https://example.org/a".to_owned()),
                DocumentId("https://example.org/b".to_owned()),
            ],
        );
        assert_eq!(
            event.scope,
            IndexMutationScope::Documents {
                document_ids: vec![
                    DocumentId("https://example.org/a".to_owned()),
                    DocumentId("https://example.org/b".to_owned()),
                ],
            }
        );
    }
}
