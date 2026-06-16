use std::sync::Arc;

use sbol_db_core::JobId;
use sbol_db_storage::{JobQueue, SbolStore};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// Context handed to every [`crate::JobHandler::run`] invocation. Carries
/// the typed service for domain operations, the job's id (for logging /
/// child enqueues), and the shutdown token the worker drove this task
/// with.
///
/// Handlers should treat the cancellation token as advisory: when it
/// fires, finish quickly and return an error so the worker can re-queue
/// the work. At-least-once semantics + the lease-expiry reaper mean
/// crashing through cancellation is also safe; it just costs more.
#[derive(Clone)]
pub struct JobContext {
    pub job_id: JobId,
    pub worker_id: Arc<str>,
    pub attempt: i32,
    pub service: Arc<dyn SbolStore>,
    pub jobs: Arc<dyn JobQueue>,
    pub cancel: CancellationToken,
}

impl JobContext {
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub async fn log(&self, level: &str, message: &str, fields: Value) {
        if let Err(err) = self
            .jobs
            .append_log(self.job_id, Some(self.attempt), level, message, fields)
            .await
        {
            tracing::warn!(error = %err, job_id = %self.job_id, "failed to append job log");
        }
    }
}
