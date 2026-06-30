//! The [`JobQueue`] implementation over the RocksDB job repository.
//!
//! The repository's methods are synchronous (they serialize on a lock); this
//! wrapper runs them on a blocking thread so the async contract holds.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sbol_db_core::{DomainError, JobId};
use sbol_db_storage::{
    EnqueueOutcome, JobAttempt, JobLogRecord, JobQueue, JobStatus, ListJobsFilter, NewJob,
    OldestQueuedAge, QueueDepthRow, SbolJob,
};
use serde_json::Value;

use crate::db::Db;
use crate::repo::JobRepository;

#[derive(Clone)]
pub struct RocksdbJobs {
    repo: Arc<JobRepository>,
}

impl RocksdbJobs {
    pub fn new(db: Db) -> Self {
        Self {
            repo: Arc::new(JobRepository::new(db)),
        }
    }
}

async fn blocking<T, F>(f: F) -> Result<T, DomainError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, DomainError> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| DomainError::Database(format!("rocksdb task panicked: {e}")))?
}

#[async_trait]
impl JobQueue for RocksdbJobs {
    async fn enqueue(&self, input: NewJob) -> Result<EnqueueOutcome, DomainError> {
        let repo = self.repo.clone();
        blocking(move || repo.enqueue(input)).await
    }

    async fn append_log(
        &self,
        job_id: JobId,
        attempt_no: Option<i32>,
        level: &str,
        message: &str,
        fields: Value,
    ) -> Result<JobLogRecord, DomainError> {
        let repo = self.repo.clone();
        let level = level.to_owned();
        let message = message.to_owned();
        blocking(move || repo.append_log(job_id, attempt_no, &level, &message, fields)).await
    }

    async fn list_logs(
        &self,
        id: JobId,
        after_id: Option<i64>,
        limit: u32,
    ) -> Result<Vec<JobLogRecord>, DomainError> {
        let repo = self.repo.clone();
        blocking(move || repo.list_logs(id, after_id, limit)).await
    }

    async fn dequeue(
        &self,
        queues: &[String],
        worker_id: &str,
        lease: Duration,
    ) -> Result<Option<SbolJob>, DomainError> {
        let repo = self.repo.clone();
        let queues = queues.to_vec();
        let worker_id = worker_id.to_owned();
        blocking(move || repo.dequeue(&queues, &worker_id, lease)).await
    }

    async fn renew_lease(
        &self,
        job_id: JobId,
        worker_id: &str,
        lease: Duration,
    ) -> Result<bool, DomainError> {
        let repo = self.repo.clone();
        let worker_id = worker_id.to_owned();
        blocking(move || repo.renew_lease(job_id, &worker_id, lease)).await
    }

    async fn mark_succeeded(
        &self,
        job_id: JobId,
        worker_id: &str,
        result: Option<Value>,
    ) -> Result<(), DomainError> {
        let repo = self.repo.clone();
        let worker_id = worker_id.to_owned();
        blocking(move || repo.mark_succeeded(job_id, &worker_id, result)).await
    }

    async fn mark_failed(
        &self,
        job_id: JobId,
        worker_id: &str,
        error: &str,
    ) -> Result<JobStatus, DomainError> {
        let repo = self.repo.clone();
        let worker_id = worker_id.to_owned();
        let error = error.to_owned();
        blocking(move || repo.mark_failed(job_id, &worker_id, &error)).await
    }

    async fn reap_expired_leases(&self) -> Result<u64, DomainError> {
        let repo = self.repo.clone();
        blocking(move || repo.reap_expired_leases()).await
    }

    async fn get(&self, id: JobId) -> Result<Option<SbolJob>, DomainError> {
        let repo = self.repo.clone();
        blocking(move || repo.get(id)).await
    }

    async fn list_attempts(&self, id: JobId) -> Result<Vec<JobAttempt>, DomainError> {
        let repo = self.repo.clone();
        blocking(move || repo.list_attempts(id)).await
    }

    async fn list(&self, filter: &ListJobsFilter) -> Result<Vec<SbolJob>, DomainError> {
        let repo = self.repo.clone();
        let filter = filter.clone();
        blocking(move || repo.list(&filter)).await
    }

    async fn cancel(&self, id: JobId) -> Result<bool, DomainError> {
        let repo = self.repo.clone();
        blocking(move || repo.cancel(id)).await
    }

    async fn current_status(&self, id: JobId) -> Result<Option<JobStatus>, DomainError> {
        let repo = self.repo.clone();
        blocking(move || repo.current_status(id)).await
    }

    async fn queue_depth_snapshot(&self) -> Result<Vec<QueueDepthRow>, DomainError> {
        let repo = self.repo.clone();
        blocking(move || repo.queue_depth_snapshot()).await
    }

    async fn oldest_queued_age(&self) -> Result<Vec<OldestQueuedAge>, DomainError> {
        let repo = self.repo.clone();
        blocking(move || repo.oldest_queued_age()).await
    }
}
