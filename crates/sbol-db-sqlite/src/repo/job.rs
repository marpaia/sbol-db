//! The async job queue over SQLite.
//!
//! Dequeue is a single atomic `UPDATE ... RETURNING` whose subquery picks the
//! next ready job; SQLite serializes writers, so this needs no `SKIP LOCKED`.
//! Timestamps are bound as `DateTime<Utc>` throughout so RFC3339 TEXT
//! comparisons (`available_at <= now`) order correctly.

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sbol_db_core::{DomainError, JobId};
use serde_json::Value;
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};

use sbol_db_storage::{
    EnqueueOutcome, JobAttempt, JobLogRecord, JobQueue, JobStatus, ListJobsFilter, NewJob,
    OldestQueuedAge, QueueDepthRow, SbolJob, DEFAULT_QUEUE,
};

use crate::pool::db_err;

const JOB_COLS: &str = "id, kind, status, priority, queue, payload, result, error, \
    idempotency_key, attempts, max_attempts, available_at, leased_by, lease_expires_at, \
    parent_job_id, correlation_id, created_at, started_at, finished_at";

#[derive(Clone)]
pub struct SqliteJobRepository {
    pool: SqlitePool,
}

impl SqliteJobRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn find_by_idempotency(
        &self,
        kind: &str,
        key: &str,
    ) -> Result<Option<SbolJob>, DomainError> {
        let row = sqlx::query(&format!(
            "SELECT {JOB_COLS} FROM sbol_jobs \
             WHERE kind = ? AND idempotency_key = ? \
               AND status IN ('queued', 'running', 'succeeded') \
             ORDER BY created_at DESC LIMIT 1"
        ))
        .bind(kind)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(row_to_job).transpose()
    }

    async fn record_attempt_started(
        &self,
        job_id: JobId,
        attempt_no: i32,
        worker_id: &str,
    ) -> Result<(), DomainError> {
        sqlx::query(
            "INSERT OR IGNORE INTO sbol_job_attempts \
             (job_id, attempt_no, worker_id, status, started_at) \
             VALUES (?, ?, ?, 'running', ?)",
        )
        .bind(job_id.as_uuid().to_string())
        .bind(attempt_no)
        .bind(worker_id)
        .bind(Utc::now())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }
}

#[async_trait]
impl JobQueue for SqliteJobRepository {
    async fn enqueue(&self, input: NewJob) -> Result<EnqueueOutcome, DomainError> {
        if let Some(key) = &input.idempotency_key {
            if let Some(existing) = self.find_by_idempotency(&input.kind, key).await? {
                return Ok(EnqueueOutcome::AlreadyExists(existing));
            }
        }

        let now = Utc::now();
        let available_at = input.available_at.unwrap_or(now);
        let queue = input.queue.unwrap_or_else(|| DEFAULT_QUEUE.to_owned());
        let priority = input.priority.unwrap_or(0) as i64;
        let max_attempts = input.max_attempts.unwrap_or(5) as i64;
        let payload = serde_json::to_string(&input.payload).map_err(db_err)?;

        let row = sqlx::query(&format!(
            r#"
            INSERT INTO sbol_jobs (
                id, kind, status, priority, queue, payload, idempotency_key,
                attempts, max_attempts, available_at, parent_job_id,
                correlation_id, created_at
            ) VALUES (?, ?, 'queued', ?, ?, ?, ?, 0, ?, ?, ?, ?, ?)
            RETURNING {JOB_COLS}
            "#
        ))
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&input.kind)
        .bind(priority)
        .bind(&queue)
        .bind(payload)
        .bind(input.idempotency_key.as_deref())
        .bind(max_attempts)
        .bind(available_at)
        .bind(input.parent_job_id.map(|j| j.as_uuid().to_string()))
        .bind(input.correlation_id.map(|c| c.to_string()))
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(EnqueueOutcome::Inserted(row_to_job(row)?))
    }

    async fn append_log(
        &self,
        job_id: JobId,
        attempt_no: Option<i32>,
        level: &str,
        message: &str,
        fields: Value,
    ) -> Result<JobLogRecord, DomainError> {
        let fields_s = serde_json::to_string(&fields).map_err(db_err)?;
        let row = sqlx::query(
            "INSERT INTO sbol_job_logs (job_id, attempt_no, level, message, fields, created_at) \
             VALUES (?, ?, ?, ?, ?, ?) \
             RETURNING id, job_id, attempt_no, level, message, fields, created_at",
        )
        .bind(job_id.as_uuid().to_string())
        .bind(attempt_no)
        .bind(normalize_log_level(level))
        .bind(message)
        .bind(fields_s)
        .bind(Utc::now())
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        row_to_job_log(row)
    }

    async fn list_logs(
        &self,
        id: JobId,
        after_id: Option<i64>,
        limit: u32,
    ) -> Result<Vec<JobLogRecord>, DomainError> {
        let rows = sqlx::query(
            "SELECT id, job_id, attempt_no, level, message, fields, created_at \
             FROM sbol_job_logs WHERE job_id = ? AND id > ? ORDER BY id ASC LIMIT ?",
        )
        .bind(id.as_uuid().to_string())
        .bind(after_id.unwrap_or(0))
        .bind(limit.clamp(1, 1000) as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter().map(row_to_job_log).collect()
    }

    async fn dequeue(
        &self,
        queues: &[String],
        worker_id: &str,
        lease: Duration,
    ) -> Result<Option<SbolJob>, DomainError> {
        if queues.is_empty() {
            return Ok(None);
        }
        let now = Utc::now();
        let lease_expires = now
            + chrono::Duration::from_std(lease).unwrap_or_else(|_| chrono::Duration::seconds(60));

        let mut qb: QueryBuilder<Sqlite> =
            QueryBuilder::new("UPDATE sbol_jobs SET status = 'running', leased_by = ");
        qb.push_bind(worker_id.to_owned());
        qb.push(", lease_expires_at = ").push_bind(lease_expires);
        qb.push(", attempts = attempts + 1, started_at = COALESCE(started_at, ")
            .push_bind(now)
            .push(")");
        qb.push(
            " WHERE id = (SELECT id FROM sbol_jobs WHERE status = 'queued' AND available_at <= ",
        )
        .push_bind(now);
        qb.push(" AND queue IN (");
        {
            let mut sep = qb.separated(", ");
            for q in queues {
                sep.push_bind(q.clone());
            }
        }
        qb.push(") ORDER BY priority DESC, available_at ASC LIMIT 1) RETURNING ");
        qb.push(JOB_COLS);

        let row = qb
            .build()
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

    async fn renew_lease(
        &self,
        job_id: JobId,
        worker_id: &str,
        lease: Duration,
    ) -> Result<bool, DomainError> {
        let lease_expires = Utc::now()
            + chrono::Duration::from_std(lease).unwrap_or_else(|_| chrono::Duration::seconds(60));
        let res = sqlx::query(
            "UPDATE sbol_jobs SET lease_expires_at = ? \
             WHERE id = ? AND leased_by = ? AND status = 'running'",
        )
        .bind(lease_expires)
        .bind(job_id.as_uuid().to_string())
        .bind(worker_id)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(res.rows_affected() == 1)
    }

    async fn mark_succeeded(
        &self,
        job_id: JobId,
        worker_id: &str,
        result: Option<Value>,
    ) -> Result<(), DomainError> {
        let result_s = result
            .map(|v| serde_json::to_string(&v))
            .transpose()
            .map_err(db_err)?;
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let row = sqlx::query(
            "UPDATE sbol_jobs SET status = 'succeeded', result = ?, error = NULL, \
             leased_by = NULL, lease_expires_at = NULL, finished_at = ? \
             WHERE id = ? AND leased_by = ? AND status = 'running' RETURNING attempts",
        )
        .bind(result_s)
        .bind(Utc::now())
        .bind(job_id.as_uuid().to_string())
        .bind(worker_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;

        if let Some(r) = row {
            let attempts: i64 = r.try_get("attempts").map_err(db_err)?;
            finalize_attempt(&mut tx, job_id, attempts as i32, JobStatus::Succeeded, None).await?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn mark_failed(
        &self,
        job_id: JobId,
        worker_id: &str,
        error: &str,
    ) -> Result<JobStatus, DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let row = sqlx::query(
            "SELECT attempts, max_attempts FROM sbol_jobs \
             WHERE id = ? AND leased_by = ? AND status = 'running'",
        )
        .bind(job_id.as_uuid().to_string())
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
        let attempts = row.try_get::<i64, _>("attempts").map_err(db_err)? as i32;
        let max_attempts = row.try_get::<i64, _>("max_attempts").map_err(db_err)? as i32;

        let next_status = if attempts >= max_attempts {
            JobStatus::Dead
        } else {
            JobStatus::Queued
        };

        match next_status {
            JobStatus::Queued => {
                let available_at =
                    Utc::now() + chrono::Duration::seconds(backoff_seconds(attempts));
                sqlx::query(
                    "UPDATE sbol_jobs SET status = 'queued', leased_by = NULL, \
                     lease_expires_at = NULL, available_at = ?, error = ? WHERE id = ?",
                )
                .bind(available_at)
                .bind(error)
                .bind(job_id.as_uuid().to_string())
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            JobStatus::Dead => {
                sqlx::query(
                    "UPDATE sbol_jobs SET status = 'dead', leased_by = NULL, \
                     lease_expires_at = NULL, error = ?, finished_at = ? WHERE id = ?",
                )
                .bind(error)
                .bind(Utc::now())
                .bind(job_id.as_uuid().to_string())
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            _ => unreachable!("next_status is queued or dead"),
        }

        let attempt_status = match next_status {
            JobStatus::Dead => JobStatus::Dead,
            _ => JobStatus::Failed,
        };
        finalize_attempt(&mut tx, job_id, attempts, attempt_status, Some(error)).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(next_status)
    }

    async fn reap_expired_leases(&self) -> Result<u64, DomainError> {
        let res = sqlx::query(
            "UPDATE sbol_jobs SET status = 'queued', leased_by = NULL, \
             lease_expires_at = NULL, available_at = ?, \
             error = COALESCE(error, 'lease expired') \
             WHERE status = 'running' AND lease_expires_at < ?",
        )
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(res.rows_affected())
    }

    async fn get(&self, id: JobId) -> Result<Option<SbolJob>, DomainError> {
        let row = sqlx::query(&format!("SELECT {JOB_COLS} FROM sbol_jobs WHERE id = ?"))
            .bind(id.as_uuid().to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        row.map(row_to_job).transpose()
    }

    async fn list_attempts(&self, id: JobId) -> Result<Vec<JobAttempt>, DomainError> {
        let rows = sqlx::query(
            "SELECT id, job_id, attempt_no, worker_id, started_at, finished_at, status, error \
             FROM sbol_job_attempts WHERE job_id = ? ORDER BY attempt_no DESC",
        )
        .bind(id.as_uuid().to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter()
            .map(|row| {
                Ok(JobAttempt {
                    id: row.try_get("id").map_err(db_err)?,
                    job_id: JobId(parse_uuid(row.try_get("job_id").map_err(db_err)?)?),
                    attempt_no: row.try_get::<i64, _>("attempt_no").map_err(db_err)? as i32,
                    worker_id: row.try_get("worker_id").map_err(db_err)?,
                    started_at: row.try_get("started_at").map_err(db_err)?,
                    finished_at: row.try_get("finished_at").map_err(db_err)?,
                    status: JobStatus::from_db_str(
                        row.try_get::<String, _>("status").map_err(db_err)?.as_str(),
                    )?,
                    error: row.try_get("error").map_err(db_err)?,
                })
            })
            .collect()
    }

    async fn list(&self, filter: &ListJobsFilter) -> Result<Vec<SbolJob>, DomainError> {
        let mut qb: QueryBuilder<Sqlite> =
            QueryBuilder::new(format!("SELECT {JOB_COLS} FROM sbol_jobs WHERE 1=1"));
        if let Some(kind) = &filter.kind {
            qb.push(" AND kind = ").push_bind(kind.clone());
        }
        if let Some(status) = filter.status {
            qb.push(" AND status = ").push_bind(status.as_db_str());
        }
        if let Some(queue) = &filter.queue {
            qb.push(" AND queue = ").push_bind(queue.clone());
        }
        if let Some(correlation_id) = filter.correlation_id {
            qb.push(" AND correlation_id = ")
                .push_bind(correlation_id.to_string());
        }
        if let Some(since) = filter.since {
            qb.push(" AND created_at >= ").push_bind(since);
        }
        qb.push(" ORDER BY created_at DESC, id DESC LIMIT ")
            .push_bind(filter.limit.clamp(1, 1000) as i64);
        let rows = qb.build().fetch_all(&self.pool).await.map_err(db_err)?;
        rows.into_iter().map(row_to_job).collect()
    }

    async fn cancel(&self, id: JobId) -> Result<bool, DomainError> {
        let res = sqlx::query(
            "UPDATE sbol_jobs SET status = 'cancelled', leased_by = NULL, \
             lease_expires_at = NULL, finished_at = COALESCE(finished_at, ?) \
             WHERE id = ? AND status IN ('queued', 'running')",
        )
        .bind(Utc::now())
        .bind(id.as_uuid().to_string())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(res.rows_affected() == 1)
    }

    async fn current_status(&self, id: JobId) -> Result<Option<JobStatus>, DomainError> {
        let row = sqlx::query("SELECT status FROM sbol_jobs WHERE id = ?")
            .bind(id.as_uuid().to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        row.map(|r| JobStatus::from_db_str(&r.try_get::<String, _>("status").map_err(db_err)?))
            .transpose()
    }

    async fn queue_depth_snapshot(&self) -> Result<Vec<QueueDepthRow>, DomainError> {
        let rows = sqlx::query(
            "SELECT status, queue, count(*) AS n FROM sbol_jobs \
             WHERE status IN ('queued', 'running', 'failed', 'dead') \
             GROUP BY status, queue",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter()
            .map(|r| {
                Ok(QueueDepthRow {
                    status: JobStatus::from_db_str(
                        r.try_get::<String, _>("status").map_err(db_err)?.as_str(),
                    )?,
                    queue: r.try_get("queue").map_err(db_err)?,
                    count: r.try_get("n").map_err(db_err)?,
                })
            })
            .collect()
    }

    async fn oldest_queued_age(&self) -> Result<Vec<OldestQueuedAge>, DomainError> {
        let now = Utc::now();
        let rows = sqlx::query(
            "SELECT queue, MIN(available_at) AS oldest FROM sbol_jobs \
             WHERE status = 'queued' AND available_at <= ? GROUP BY queue",
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter()
            .map(|r| {
                let oldest: DateTime<Utc> = r.try_get("oldest").map_err(db_err)?;
                Ok(OldestQueuedAge {
                    queue: r.try_get("queue").map_err(db_err)?,
                    age_secs: (now - oldest).num_milliseconds() as f64 / 1000.0,
                })
            })
            .collect()
    }
}

async fn finalize_attempt(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    job_id: JobId,
    attempt_no: i32,
    status: JobStatus,
    error: Option<&str>,
) -> Result<(), DomainError> {
    sqlx::query(
        "UPDATE sbol_job_attempts SET finished_at = ?, status = ?, error = ? \
         WHERE job_id = ? AND attempt_no = ?",
    )
    .bind(Utc::now())
    .bind(status.as_db_str())
    .bind(error)
    .bind(job_id.as_uuid().to_string())
    .bind(attempt_no)
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

fn normalize_log_level(level: &str) -> &'static str {
    match level {
        "debug" => "debug",
        "warn" => "warn",
        "error" => "error",
        _ => "info",
    }
}

fn parse_uuid(s: String) -> Result<uuid::Uuid, DomainError> {
    uuid::Uuid::parse_str(&s).map_err(db_err)
}

fn row_to_job(row: sqlx::sqlite::SqliteRow) -> Result<SbolJob, DomainError> {
    let payload: String = row.try_get("payload").map_err(db_err)?;
    let result: Option<String> = row.try_get("result").map_err(db_err)?;
    let parent_job_id: Option<String> = row.try_get("parent_job_id").map_err(db_err)?;
    let correlation_id: Option<String> = row.try_get("correlation_id").map_err(db_err)?;

    Ok(SbolJob {
        id: JobId(parse_uuid(row.try_get("id").map_err(db_err)?)?),
        kind: row.try_get("kind").map_err(db_err)?,
        status: JobStatus::from_db_str(
            row.try_get::<String, _>("status").map_err(db_err)?.as_str(),
        )?,
        priority: row.try_get::<i64, _>("priority").map_err(db_err)? as i16,
        queue: row.try_get("queue").map_err(db_err)?,
        payload: serde_json::from_str(&payload).map_err(db_err)?,
        result: result
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(db_err)?,
        error: row.try_get("error").map_err(db_err)?,
        idempotency_key: row.try_get("idempotency_key").map_err(db_err)?,
        attempts: row.try_get::<i64, _>("attempts").map_err(db_err)? as i32,
        max_attempts: row.try_get::<i64, _>("max_attempts").map_err(db_err)? as i32,
        available_at: row.try_get("available_at").map_err(db_err)?,
        leased_by: row.try_get("leased_by").map_err(db_err)?,
        lease_expires_at: row.try_get("lease_expires_at").map_err(db_err)?,
        parent_job_id: parent_job_id.map(parse_uuid).transpose()?.map(JobId),
        correlation_id: correlation_id.map(parse_uuid).transpose()?,
        created_at: row.try_get("created_at").map_err(db_err)?,
        started_at: row.try_get("started_at").map_err(db_err)?,
        finished_at: row.try_get("finished_at").map_err(db_err)?,
    })
}

fn row_to_job_log(row: sqlx::sqlite::SqliteRow) -> Result<JobLogRecord, DomainError> {
    let fields: String = row.try_get("fields").map_err(db_err)?;
    Ok(JobLogRecord {
        id: row.try_get("id").map_err(db_err)?,
        job_id: JobId(parse_uuid(row.try_get("job_id").map_err(db_err)?)?),
        attempt_no: row
            .try_get::<Option<i64>, _>("attempt_no")
            .map_err(db_err)?
            .map(|n| n as i32),
        level: row.try_get("level").map_err(db_err)?,
        message: row.try_get("message").map_err(db_err)?,
        fields: serde_json::from_str(&fields).map_err(db_err)?,
        created_at: row.try_get("created_at").map_err(db_err)?,
    })
}
