//! Job-queue request/response types.

use chrono::{DateTime, Utc};
use sbol_db_core::{DomainError, JobId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const DEFAULT_QUEUE: &str = "default";

/// Lifecycle state for a job row. `as_db_str`/`from_db_str` convert to and
/// from the persisted string form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Dead,
}

impl JobStatus {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Dead => "dead",
        }
    }

    pub fn from_db_str(s: &str) -> Result<Self, DomainError> {
        Ok(match s {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "dead" => Self::Dead,
            other => {
                return Err(DomainError::Database(format!(
                    "unknown job status: {other}"
                )))
            }
        })
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Dead
        )
    }
}

/// One job row. Carries the typed lifecycle state plus the JSON payload that
/// handlers deserialise themselves.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SbolJob {
    pub id: JobId,
    pub kind: String,
    pub status: JobStatus,
    pub priority: i16,
    pub queue: String,
    pub payload: Value,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub idempotency_key: Option<String>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub available_at: DateTime<Utc>,
    pub leased_by: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub parent_job_id: Option<JobId>,
    pub correlation_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// Insert-shape for a new job. `kind` must match a handler registered in the
/// worker's registry; the worker validates this on dequeue.
#[derive(Clone, Debug)]
pub struct NewJob {
    pub kind: String,
    pub payload: Value,
    pub queue: Option<String>,
    pub priority: Option<i16>,
    pub max_attempts: Option<i32>,
    pub idempotency_key: Option<String>,
    pub available_at: Option<DateTime<Utc>>,
    pub parent_job_id: Option<JobId>,
    pub correlation_id: Option<Uuid>,
}

impl NewJob {
    pub fn new(kind: impl Into<String>, payload: Value) -> Self {
        Self {
            kind: kind.into(),
            payload,
            queue: None,
            priority: None,
            max_attempts: None,
            idempotency_key: None,
            available_at: None,
            parent_job_id: None,
            correlation_id: None,
        }
    }
}

/// Filter for the operator listing surface (`GET /jobs`, `sbol-db jobs list`).
/// Keyset cursor is on `(created_at DESC, id)`.
#[derive(Clone, Debug, Default)]
pub struct ListJobsFilter {
    pub kind: Option<String>,
    pub status: Option<JobStatus>,
    pub queue: Option<String>,
    pub correlation_id: Option<Uuid>,
    pub since: Option<DateTime<Utc>>,
    pub limit: u32,
}

/// One bucket of the queue-depth snapshot. Cardinality is small
/// (status × queue), so this is safe to expose as Prometheus labels.
#[derive(Clone, Debug)]
pub struct QueueDepthRow {
    pub status: JobStatus,
    pub queue: String,
    pub count: i64,
}

/// Age in seconds of the oldest still-queued job per queue. Drives the
/// `sbol_db_jobs_oldest_queued_age_seconds` alerting gauge.
#[derive(Clone, Debug)]
pub struct OldestQueuedAge {
    pub queue: String,
    pub age_secs: f64,
}

/// One attempt at running a job. Each dequeue records a new attempt; failures
/// finalise the attempt with the error text before the parent job either
/// re-queues or transitions to `dead`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobAttempt {
    pub id: i64,
    pub job_id: JobId,
    pub attempt_no: i32,
    pub worker_id: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: JobStatus,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobLogRecord {
    pub id: i64,
    pub job_id: JobId,
    pub attempt_no: Option<i32>,
    pub level: String,
    pub message: String,
    pub fields: Value,
    pub created_at: DateTime<Utc>,
}

/// Outcome of an enqueue when the row already existed under a matching
/// `idempotency_key`. Callers usually treat both arms identically — the
/// existing row is what they wanted anyway.
#[derive(Clone, Debug)]
pub enum EnqueueOutcome {
    Inserted(SbolJob),
    AlreadyExists(SbolJob),
}

impl EnqueueOutcome {
    pub fn into_job(self) -> SbolJob {
        match self {
            Self::Inserted(j) | Self::AlreadyExists(j) => j,
        }
    }
}
