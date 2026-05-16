use std::time::Duration;

use chrono::{DateTime, Utc};
use sbol_db_core::{DomainError, JobId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use crate::repo::db_err;
use crate::PgPool;

pub const DEFAULT_QUEUE: &str = "default";

/// Lifecycle state for an [`SbolJob`] row. Mirrors the `sbol_job_status`
/// Postgres enum.
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

/// One row in `sbol_jobs`. Carries the typed lifecycle state plus the
/// JSON payload that handlers deserialise themselves.
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

/// Insert-shape for a new job. `kind` must match a handler registered in
/// the worker's [`JobRegistry`]; the worker validates this on dequeue.
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

/// Filter for the `list_jobs` operator surface (`GET /jobs`, `sbol-db
/// jobs list`). Keyset cursor is on `(created_at DESC, id)`.
#[derive(Clone, Debug, Default)]
pub struct ListJobsFilter {
    pub kind: Option<String>,
    pub status: Option<JobStatus>,
    pub queue: Option<String>,
    pub correlation_id: Option<Uuid>,
    pub since: Option<DateTime<Utc>>,
    pub limit: u32,
}

/// One bucket of the queue-depth snapshot returned by
/// [`JobRepository::queue_depth_snapshot`]. Cardinality is small
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

/// Outcome of [`JobRepository::enqueue`] when the row already existed
/// under a matching `idempotency_key`. Callers usually treat both arms
/// identically — the existing row is what they wanted anyway.
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

#[derive(Clone)]
pub struct JobRepository {
    pool: PgPool,
}

impl JobRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Insert a job. When an `idempotency_key` is set and a non-terminal
    /// (or succeeded) row already exists for `(kind, key)`, returns the
    /// existing row rather than inserting a duplicate.
    pub async fn enqueue(&self, input: NewJob) -> Result<EnqueueOutcome, DomainError> {
        if let Some(key) = &input.idempotency_key {
            if let Some(existing) = self.find_by_idempotency(&input.kind, key).await? {
                return Ok(EnqueueOutcome::AlreadyExists(existing));
            }
        }

        let queue = input
            .queue
            .clone()
            .unwrap_or_else(|| DEFAULT_QUEUE.to_owned());
        let priority = input.priority.unwrap_or(0);
        let max_attempts = input.max_attempts.unwrap_or(5);

        let row = sqlx::query(
            r#"
            INSERT INTO sbol_jobs (
                kind, queue, priority, payload,
                idempotency_key, max_attempts, available_at,
                parent_job_id, correlation_id
            ) VALUES (
                $1, $2, $3, $4,
                $5, $6, COALESCE($7, now()),
                $8, $9
            )
            RETURNING id, kind, status::text AS status, priority, queue, payload,
                      result, error, idempotency_key, attempts, max_attempts,
                      available_at, leased_by, lease_expires_at,
                      parent_job_id, correlation_id,
                      created_at, started_at, finished_at
            "#,
        )
        .bind(&input.kind)
        .bind(&queue)
        .bind(priority)
        .bind(&input.payload)
        .bind(input.idempotency_key.as_deref())
        .bind(max_attempts)
        .bind(input.available_at)
        .bind(input.parent_job_id.map(|j| j.as_uuid()))
        .bind(input.correlation_id)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;

        Ok(EnqueueOutcome::Inserted(row_to_job(row)?))
    }

    async fn find_by_idempotency(
        &self,
        kind: &str,
        key: &str,
    ) -> Result<Option<SbolJob>, DomainError> {
        let row = sqlx::query(
            r#"
            SELECT id, kind, status::text AS status, priority, queue, payload,
                   result, error, idempotency_key, attempts, max_attempts,
                   available_at, leased_by, lease_expires_at,
                   parent_job_id, correlation_id,
                   created_at, started_at, finished_at
            FROM sbol_jobs
            WHERE kind = $1
              AND idempotency_key = $2
              AND status IN ('queued', 'running', 'succeeded')
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(kind)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(row_to_job).transpose()
    }

    /// Dequeue one job from the given queues, taking a lease for the
    /// worker. Uses `FOR UPDATE SKIP LOCKED` so concurrent workers across
    /// the cluster never contend on the same row.
    pub async fn dequeue(
        &self,
        queues: &[String],
        worker_id: &str,
        lease: Duration,
    ) -> Result<Option<SbolJob>, DomainError> {
        let lease_interval = format!("{} seconds", lease.as_secs_f64());
        let row = sqlx::query(
            r#"
            WITH next AS (
                SELECT id
                FROM sbol_jobs
                WHERE status = 'queued'
                  AND queue = ANY($1)
                  AND available_at <= now()
                ORDER BY priority DESC, available_at
                LIMIT 1
                FOR UPDATE SKIP LOCKED
            )
            UPDATE sbol_jobs j
               SET status = 'running',
                   leased_by = $2,
                   lease_expires_at = now() + $3::interval,
                   attempts = j.attempts + 1,
                   started_at = COALESCE(j.started_at, now())
              FROM next
             WHERE j.id = next.id
            RETURNING j.id, j.kind, j.status::text AS status, j.priority, j.queue, j.payload,
                      j.result, j.error, j.idempotency_key, j.attempts, j.max_attempts,
                      j.available_at, j.leased_by, j.lease_expires_at,
                      j.parent_job_id, j.correlation_id,
                      j.created_at, j.started_at, j.finished_at
            "#,
        )
        .bind(queues)
        .bind(worker_id)
        .bind(lease_interval)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;

        let job = row.map(row_to_job).transpose()?;
        if let Some(j) = &job {
            self.record_attempt_started(j.id, j.attempts, worker_id)
                .await?;
        }
        Ok(job)
    }

    async fn record_attempt_started(
        &self,
        job_id: JobId,
        attempt_no: i32,
        worker_id: &str,
    ) -> Result<(), DomainError> {
        sqlx::query(
            r#"
            INSERT INTO sbol_job_attempts (job_id, attempt_no, worker_id, status)
            VALUES ($1, $2, $3, 'running')
            ON CONFLICT (job_id, attempt_no) DO NOTHING
            "#,
        )
        .bind(job_id.as_uuid())
        .bind(attempt_no)
        .bind(worker_id)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// Extend the lease on a running job. Returns `false` when the lease
    /// has already been stolen (different `leased_by`) or the job has
    /// transitioned out of `running`.
    pub async fn renew_lease(
        &self,
        job_id: JobId,
        worker_id: &str,
        lease: Duration,
    ) -> Result<bool, DomainError> {
        let lease_interval = format!("{} seconds", lease.as_secs_f64());
        let res = sqlx::query(
            r#"
            UPDATE sbol_jobs
               SET lease_expires_at = now() + $3::interval
             WHERE id = $1
               AND leased_by = $2
               AND status = 'running'
            "#,
        )
        .bind(job_id.as_uuid())
        .bind(worker_id)
        .bind(lease_interval)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(res.rows_affected() == 1)
    }

    pub async fn mark_succeeded(
        &self,
        job_id: JobId,
        worker_id: &str,
        result: Option<Value>,
    ) -> Result<(), DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let row = sqlx::query(
            r#"
            UPDATE sbol_jobs
               SET status = 'succeeded',
                   result = $3,
                   error = NULL,
                   leased_by = NULL,
                   lease_expires_at = NULL,
                   finished_at = now()
             WHERE id = $1
               AND leased_by = $2
               AND status = 'running'
            RETURNING attempts
            "#,
        )
        .bind(job_id.as_uuid())
        .bind(worker_id)
        .bind(result)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;

        if let Some(r) = row {
            let attempts: i32 = r.try_get("attempts").map_err(db_err)?;
            finalize_attempt(&mut tx, job_id, attempts, JobStatus::Succeeded, None).await?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    /// Mark a failed attempt. If the job still has retries left, transition
    /// it back to `queued` with exponential backoff (`60s * 2^attempts`,
    /// capped at 1 hour). Otherwise it lands in `dead`.
    pub async fn mark_failed(
        &self,
        job_id: JobId,
        worker_id: &str,
        error: &str,
    ) -> Result<JobStatus, DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let row = sqlx::query(
            r#"
            SELECT attempts, max_attempts
            FROM sbol_jobs
            WHERE id = $1 AND leased_by = $2 AND status = 'running'
            FOR UPDATE
            "#,
        )
        .bind(job_id.as_uuid())
        .bind(worker_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;

        let Some(row) = row else {
            tx.commit().await.map_err(db_err)?;
            return Err(DomainError::Database(
                "job lease lost before failure could be recorded".to_owned(),
            ));
        };
        let attempts: i32 = row.try_get("attempts").map_err(db_err)?;
        let max_attempts: i32 = row.try_get("max_attempts").map_err(db_err)?;

        let next_status = if attempts >= max_attempts {
            JobStatus::Dead
        } else {
            JobStatus::Queued
        };

        match next_status {
            JobStatus::Queued => {
                let backoff_secs = backoff_seconds(attempts);
                sqlx::query(
                    r#"
                    UPDATE sbol_jobs
                       SET status = 'queued',
                           leased_by = NULL,
                           lease_expires_at = NULL,
                           available_at = now() + ($2 || ' seconds')::interval,
                           error = $3
                     WHERE id = $1
                    "#,
                )
                .bind(job_id.as_uuid())
                .bind(backoff_secs.to_string())
                .bind(error)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            JobStatus::Dead => {
                sqlx::query(
                    r#"
                    UPDATE sbol_jobs
                       SET status = 'dead',
                           leased_by = NULL,
                           lease_expires_at = NULL,
                           error = $2,
                           finished_at = now()
                     WHERE id = $1
                    "#,
                )
                .bind(job_id.as_uuid())
                .bind(error)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            _ => unreachable!("next_status is queued or dead"),
        }
        let _ = worker_id;

        let attempt_status = match next_status {
            JobStatus::Dead => JobStatus::Dead,
            _ => JobStatus::Failed,
        };
        finalize_attempt(&mut tx, job_id, attempts, attempt_status, Some(error)).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(next_status)
    }

    /// Re-queue jobs whose lease has expired. Workers run this on a slow
    /// timer; one losing the race is harmless — `UPDATE ... WHERE status =
    /// 'running'` is idempotent under concurrent reapers.
    pub async fn reap_expired_leases(&self) -> Result<u64, DomainError> {
        let res = sqlx::query(
            r#"
            UPDATE sbol_jobs
               SET status = 'queued',
                   leased_by = NULL,
                   lease_expires_at = NULL,
                   available_at = now(),
                   error = COALESCE(error, 'lease expired')
             WHERE status = 'running'
               AND lease_expires_at < now()
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(res.rows_affected())
    }

    pub async fn get(&self, id: JobId) -> Result<Option<SbolJob>, DomainError> {
        let row = sqlx::query(
            r#"
            SELECT id, kind, status::text AS status, priority, queue, payload,
                   result, error, idempotency_key, attempts, max_attempts,
                   available_at, leased_by, lease_expires_at,
                   parent_job_id, correlation_id,
                   created_at, started_at, finished_at
            FROM sbol_jobs
            WHERE id = $1
            "#,
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(row_to_job).transpose()
    }

    pub async fn list(&self, filter: &ListJobsFilter) -> Result<Vec<SbolJob>, DomainError> {
        let limit = filter.limit.clamp(1, 1000) as i64;
        let rows = sqlx::query(
            r#"
            SELECT id, kind, status::text AS status, priority, queue, payload,
                   result, error, idempotency_key, attempts, max_attempts,
                   available_at, leased_by, lease_expires_at,
                   parent_job_id, correlation_id,
                   created_at, started_at, finished_at
            FROM sbol_jobs
            WHERE ($1::text IS NULL OR kind = $1)
              AND ($2::text IS NULL OR status::text = $2)
              AND ($3::text IS NULL OR queue = $3)
              AND ($4::uuid IS NULL OR correlation_id = $4)
              AND ($5::timestamptz IS NULL OR created_at >= $5)
            ORDER BY created_at DESC, id DESC
            LIMIT $6
            "#,
        )
        .bind(filter.kind.as_deref())
        .bind(filter.status.map(|s| s.as_db_str()))
        .bind(filter.queue.as_deref())
        .bind(filter.correlation_id)
        .bind(filter.since)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter().map(row_to_job).collect()
    }

    /// Cancel a job. Only `queued` jobs cancel cleanly; `running` jobs are
    /// marked `cancelled` but the worker still finishes its current step
    /// (cooperative cancellation is a v2 concern).
    pub async fn cancel(&self, id: JobId) -> Result<bool, DomainError> {
        let res = sqlx::query(
            r#"
            UPDATE sbol_jobs
               SET status = 'cancelled',
                   leased_by = NULL,
                   lease_expires_at = NULL,
                   finished_at = COALESCE(finished_at, now())
             WHERE id = $1
               AND status IN ('queued', 'running')
            "#,
        )
        .bind(id.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(res.rows_affected() == 1)
    }

    /// Used by handler dispatch and the worker's heartbeat: peek the
    /// status without taking a lease.
    pub async fn current_status(&self, id: JobId) -> Result<Option<JobStatus>, DomainError> {
        let row = sqlx::query("SELECT status::text AS status FROM sbol_jobs WHERE id = $1")
            .bind(id.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        row.map(|r| {
            let s: String = r.try_get("status").map_err(db_err)?;
            JobStatus::from_db_str(&s)
        })
        .transpose()
    }

    /// Aggregate row count grouped by `(status, queue)` for the
    /// non-terminal lifecycle states. Drives the `/metrics`
    /// `sbol_db_jobs_queue_depth` gauge. Cheap given the partial dequeue
    /// index covers `status = 'queued'`.
    pub async fn queue_depth_snapshot(&self) -> Result<Vec<QueueDepthRow>, DomainError> {
        let rows = sqlx::query(
            r#"
            SELECT status::text AS status, queue, count(*)::bigint AS n
            FROM sbol_jobs
            WHERE status IN ('queued', 'running', 'failed', 'dead')
            GROUP BY status, queue
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter()
            .map(|r| {
                let status_str: String = r.try_get("status").map_err(db_err)?;
                let queue: String = r.try_get("queue").map_err(db_err)?;
                let n: i64 = r.try_get("n").map_err(db_err)?;
                Ok(QueueDepthRow {
                    status: JobStatus::from_db_str(&status_str)?,
                    queue,
                    count: n,
                })
            })
            .collect()
    }

    /// Oldest `available_at` per queue among `queued` rows whose
    /// `available_at <= now()`. Lets ops alert on stalled queues without
    /// chasing individual job ids.
    pub async fn oldest_queued_age(&self) -> Result<Vec<OldestQueuedAge>, DomainError> {
        let rows = sqlx::query(
            r#"
            SELECT queue, EXTRACT(EPOCH FROM (now() - MIN(available_at)))::float8 AS age_secs
            FROM sbol_jobs
            WHERE status = 'queued' AND available_at <= now()
            GROUP BY queue
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter()
            .map(|r| {
                let queue: String = r.try_get("queue").map_err(db_err)?;
                let age_secs: f64 = r.try_get("age_secs").map_err(db_err)?;
                Ok(OldestQueuedAge { queue, age_secs })
            })
            .collect()
    }
}

async fn finalize_attempt(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: JobId,
    attempt_no: i32,
    status: JobStatus,
    error: Option<&str>,
) -> Result<(), DomainError> {
    sqlx::query(
        r#"
        UPDATE sbol_job_attempts
           SET finished_at = now(),
               status = $3::sbol_job_status,
               error = $4
         WHERE job_id = $1 AND attempt_no = $2
        "#,
    )
    .bind(job_id.as_uuid())
    .bind(attempt_no)
    .bind(status.as_db_str())
    .bind(error)
    .execute(&mut **tx)
    .await
    .map_err(db_err)?;
    Ok(())
}

fn backoff_seconds(attempts: i32) -> i64 {
    let attempts = attempts.max(1);
    let base: i64 = 60;
    let cap: i64 = 60 * 60;
    let shift = attempts.saturating_sub(1).min(6) as u32;
    base.saturating_mul(1i64 << shift).min(cap)
}

fn row_to_job(row: sqlx::postgres::PgRow) -> Result<SbolJob, DomainError> {
    let id: Uuid = row.try_get("id").map_err(db_err)?;
    let kind: String = row.try_get("kind").map_err(db_err)?;
    let status: String = row.try_get("status").map_err(db_err)?;
    let priority: i16 = row.try_get("priority").map_err(db_err)?;
    let queue: String = row.try_get("queue").map_err(db_err)?;
    let payload: Value = row.try_get("payload").map_err(db_err)?;
    let result: Option<Value> = row.try_get("result").map_err(db_err)?;
    let error: Option<String> = row.try_get("error").map_err(db_err)?;
    let idempotency_key: Option<String> = row.try_get("idempotency_key").map_err(db_err)?;
    let attempts: i32 = row.try_get("attempts").map_err(db_err)?;
    let max_attempts: i32 = row.try_get("max_attempts").map_err(db_err)?;
    let available_at: DateTime<Utc> = row.try_get("available_at").map_err(db_err)?;
    let leased_by: Option<String> = row.try_get("leased_by").map_err(db_err)?;
    let lease_expires_at: Option<DateTime<Utc>> =
        row.try_get("lease_expires_at").map_err(db_err)?;
    let parent_job_id: Option<Uuid> = row.try_get("parent_job_id").map_err(db_err)?;
    let correlation_id: Option<Uuid> = row.try_get("correlation_id").map_err(db_err)?;
    let created_at: DateTime<Utc> = row.try_get("created_at").map_err(db_err)?;
    let started_at: Option<DateTime<Utc>> = row.try_get("started_at").map_err(db_err)?;
    let finished_at: Option<DateTime<Utc>> = row.try_get("finished_at").map_err(db_err)?;

    Ok(SbolJob {
        id: JobId(id),
        kind,
        status: JobStatus::from_db_str(&status)?,
        priority,
        queue,
        payload,
        result,
        error,
        idempotency_key,
        attempts,
        max_attempts,
        available_at,
        leased_by,
        lease_expires_at,
        parent_job_id: parent_job_id.map(JobId),
        correlation_id,
        created_at,
        started_at,
        finished_at,
    })
}
