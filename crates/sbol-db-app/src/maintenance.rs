//! Application-side dispatch of plugin-defined search maintenance.
//!
//! Search plugins provide storage-neutral task plans through the SDK. This
//! adapter is the one place that knows how to turn those plans into durable
//! sbol-db jobs after a data write has committed.

use std::sync::Arc;

use sbol_db_core::DomainError;
use sbol_db_search_sdk::{IndexMaintenanceEvent, IndexMaintenanceRegistry};
use sbol_db_storage::{EnqueueOutcome, JobQueue, NewJob};

/// Result of submitting all maintenance tasks for one committed application
/// write. The write is not included here because it has already committed when
/// this receipt is produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MaintenanceScheduleReceipt {
    pub enqueued: usize,
    pub deduplicated: usize,
}

/// Bridges SDK maintenance plugins to the application's durable job queue.
#[derive(Clone)]
pub struct SearchMaintenanceScheduler {
    jobs: Arc<dyn JobQueue>,
    plugins: Arc<IndexMaintenanceRegistry>,
}

impl SearchMaintenanceScheduler {
    pub fn new(jobs: Arc<dyn JobQueue>, plugins: Arc<IndexMaintenanceRegistry>) -> Self {
        Self { jobs, plugins }
    }

    pub fn is_enabled(&self) -> bool {
        !self.plugins.is_empty()
    }

    /// Plan every plugin before enqueueing any task, then submit the resulting
    /// desired-state jobs in stable plugin order. A scheduling failure is
    /// surfaced to the caller rather than silently claiming that an index will
    /// converge; the authoritative write has already committed and can be
    /// repaired by replaying or rebuilding the index.
    pub async fn schedule(
        &self,
        event: IndexMaintenanceEvent,
    ) -> Result<MaintenanceScheduleReceipt, DomainError> {
        let mut tasks = Vec::new();
        for plugin in self.plugins.plugins() {
            let planned = plugin.plan(&event).await.map_err(|error| {
                DomainError::Unavailable(format!(
                    "planning search maintenance with plugin {:?}: {error}",
                    plugin.descriptor().id
                ))
            })?;
            tasks.extend(planned);
        }

        let mut receipt = MaintenanceScheduleReceipt::default();
        for task in tasks {
            if task.kind.trim().is_empty() {
                return Err(DomainError::InvalidInput(
                    "index maintenance plugin produced an empty job kind".to_owned(),
                ));
            }
            match self
                .jobs
                .enqueue(NewJob {
                    kind: task.kind,
                    payload: task.payload,
                    queue: task.queue,
                    priority: task.priority,
                    max_attempts: task.max_attempts,
                    idempotency_key: task.idempotency_key,
                    available_at: None,
                    parent_job_id: None,
                    correlation_id: None,
                })
                .await?
            {
                EnqueueOutcome::Inserted(_) => receipt.enqueued += 1,
                EnqueueOutcome::AlreadyExists(_) => receipt.deduplicated += 1,
            }
        }
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use sbol_db_backend::Backend;
    use sbol_db_search_sdk::{
        IndexMaintenanceDescriptor, IndexMaintenancePlugin, IndexMaintenanceRegistry,
        IndexMaintenanceTask, IndexMutationSource, SearchError,
    };
    use sbol_db_storage::ListJobsFilter;

    use super::*;

    struct ProbePlugin {
        descriptor: IndexMaintenanceDescriptor,
    }

    #[async_trait]
    impl IndexMaintenancePlugin for ProbePlugin {
        fn descriptor(&self) -> &IndexMaintenanceDescriptor {
            &self.descriptor
        }

        async fn plan(
            &self,
            _event: &IndexMaintenanceEvent,
        ) -> Result<Vec<IndexMaintenanceTask>, SearchError> {
            Ok(vec![IndexMaintenanceTask::new(
                "probe_index_maintenance",
                serde_json::json!({"source": "test"}),
            )])
        }
    }

    #[tokio::test]
    async fn dispatches_plugin_tasks_to_the_durable_job_queue() {
        let directory = tempfile::tempdir().unwrap();
        let database_url = format!("sqlite://{}", directory.path().join("jobs.db").display());
        let backend = Backend::open(&database_url).await.unwrap();
        backend
            .migrator
            .as_ref()
            .unwrap()
            .run_migrations()
            .await
            .unwrap();
        let plugins = IndexMaintenanceRegistry::builder()
            .register(ProbePlugin {
                descriptor: IndexMaintenanceDescriptor {
                    id: "probe.maintenance.v1".to_owned(),
                    display_name: "Probe".to_owned(),
                    description: "test plugin".to_owned(),
                },
            })
            .unwrap()
            .build();
        let scheduler = SearchMaintenanceScheduler::new(backend.jobs.clone(), Arc::new(plugins));

        let receipt = scheduler
            .schedule(IndexMaintenanceEvent::corpus(
                IndexMutationSource::RestImport,
            ))
            .await
            .unwrap();
        assert_eq!(receipt.enqueued, 1);
        let jobs = backend
            .jobs
            .list(&ListJobsFilter {
                kind: Some("probe_index_maintenance".to_owned()),
                limit: 10,
                ..ListJobsFilter::default()
            })
            .await
            .unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].payload["source"], "test");
    }
}
