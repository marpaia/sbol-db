//! The job queue over RocksDB.
//!
//! Jobs are stored by id; a `job_ready` ordering index holds exactly the queued
//! jobs, keyed `queue ++ priority ++ available_at ++ id` so a dequeue is a
//! prefix scan that yields the highest-priority available job first. Mutating
//! operations take a process-wide lock so the read-modify-write of a claim or
//! transition is atomic; reads are lock-free. Attempt and log ids come from
//! monotonic counters in the `meta` family.

use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rocksdb::WriteBatch;
use sbol_db_core::{DomainError, JobId};
use sbol_db_storage::{
    EnqueueOutcome, JobAttempt, JobLogRecord, JobStatus, ListJobsFilter, NewJob, OldestQueuedAge,
    QueueDepthRow, SbolJob, DEFAULT_QUEUE,
};
use serde_json::Value;
use uuid::Uuid;

use crate::db::{compose, Db};

const DEFAULT_MAX_ATTEMPTS: i32 = 5;
const LIST_LIMIT_MAX: u32 = 1000;
const LOG_LIMIT_MAX: u32 = 1000;

pub struct JobRepository {
    db: Db,
    /// Serializes claims and lifecycle transitions so each read-modify-write is
    /// atomic. The triplestore's hot path never touches this.
    write_lock: Mutex<()>,
}

impl JobRepository {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            write_lock: Mutex::new(()),
        }
    }

    // --- enqueue -----------------------------------------------------------

    pub fn enqueue(&self, input: NewJob) -> Result<EnqueueOutcome, DomainError> {
        let _guard = self.lock();
        if let Some(key) = &input.idempotency_key {
            if let Some(existing) = self.find_active_by_idempotency(&input.kind, key)? {
                return Ok(EnqueueOutcome::AlreadyExists(existing));
            }
        }

        let now = Utc::now();
        let job = SbolJob {
            id: JobId::new(),
            kind: input.kind.clone(),
            status: JobStatus::Queued,
            priority: input.priority.unwrap_or(0),
            queue: input
                .queue
                .clone()
                .unwrap_or_else(|| DEFAULT_QUEUE.to_owned()),
            payload: input.payload.clone(),
            result: None,
            error: None,
            idempotency_key: input.idempotency_key.clone(),
            attempts: 0,
            max_attempts: input.max_attempts.unwrap_or(DEFAULT_MAX_ATTEMPTS),
            available_at: input.available_at.unwrap_or(now),
            leased_by: None,
            lease_expires_at: None,
            parent_job_id: input.parent_job_id,
            correlation_id: input.correlation_id,
            created_at: now,
            started_at: None,
            finished_at: None,
        };

        let mut batch = WriteBatch::default();
        self.stage_put_job(&mut batch, &job)?;
        self.stage_add_ready(&mut batch, &job);
        if let Some(key) = &input.idempotency_key {
            batch.put_cf(
                &self.db.cf("job_idem"),
                idem_key(&input.kind, key),
                job.id.0.as_bytes(),
            );
        }
        self.db.write(batch)?;
        Ok(EnqueueOutcome::Inserted(job))
    }

    fn find_active_by_idempotency(
        &self,
        kind: &str,
        key: &str,
    ) -> Result<Option<SbolJob>, DomainError> {
        let Some(id_bytes) = self.db.get_cf("job_idem", &idem_key(kind, key))? else {
            return Ok(None);
        };
        let id = job_id_from_bytes(&id_bytes)?;
        match self.get(id)? {
            Some(job)
                if matches!(
                    job.status,
                    JobStatus::Queued | JobStatus::Running | JobStatus::Succeeded
                ) =>
            {
                Ok(Some(job))
            }
            _ => Ok(None),
        }
    }

    // --- dequeue + lease ---------------------------------------------------

    pub fn dequeue(
        &self,
        queues: &[String],
        worker_id: &str,
        lease: Duration,
    ) -> Result<Option<SbolJob>, DomainError> {
        let _guard = self.lock();
        let now = Utc::now();
        let now_millis = now.timestamp_millis();

        // Best available candidate across the requested queues, ordered by the
        // ready-index suffix (priority desc, then available_at asc).
        let mut best: Option<(Vec<u8>, JobId)> = None;
        for queue in queues {
            let mut prefix = queue.as_bytes().to_vec();
            prefix.push(crate::db::SEP);
            let plen = prefix.len();
            let mut found: Option<(Vec<u8>, JobId)> = None;
            self.db.for_each_prefix("job_ready", &prefix, |key, _| {
                let suffix = &key[plen..];
                let avail = decode_avail(&suffix[2..10]);
                if avail > now_millis {
                    return Ok(true); // not yet available; keep scanning
                }
                let id = job_id_from_bytes(&suffix[10..26])?;
                found = Some((suffix[..10].to_vec(), id));
                Ok(false)
            })?;
            if let Some((order, id)) = found {
                let better = match &best {
                    Some((b, _)) => order < *b,
                    None => true,
                };
                if better {
                    best = Some((order, id));
                }
            }
        }

        let Some((_, id)) = best else {
            return Ok(None);
        };
        let Some(mut job) = self.get(id)? else {
            return Ok(None);
        };
        if job.status != JobStatus::Queued {
            return Ok(None);
        }

        let mut batch = WriteBatch::default();
        self.stage_remove_ready(&mut batch, &job);

        job.attempts += 1;
        job.status = JobStatus::Running;
        job.leased_by = Some(worker_id.to_owned());
        job.lease_expires_at = Some(now + lease_duration(lease));
        if job.started_at.is_none() {
            job.started_at = Some(now);
        }
        self.stage_put_job(&mut batch, &job)?;
        self.stage_record_attempt_started(&mut batch, &job, worker_id, now)?;
        self.db.write(batch)?;
        Ok(Some(job))
    }

    pub fn renew_lease(
        &self,
        job_id: JobId,
        worker_id: &str,
        lease: Duration,
    ) -> Result<bool, DomainError> {
        let _guard = self.lock();
        let Some(mut job) = self.get(job_id)? else {
            return Ok(false);
        };
        if job.status != JobStatus::Running || job.leased_by.as_deref() != Some(worker_id) {
            return Ok(false);
        }
        job.lease_expires_at = Some(Utc::now() + lease_duration(lease));
        let mut batch = WriteBatch::default();
        self.stage_put_job(&mut batch, &job)?;
        self.db.write(batch)?;
        Ok(true)
    }

    pub fn mark_succeeded(
        &self,
        job_id: JobId,
        worker_id: &str,
        result: Option<Value>,
    ) -> Result<(), DomainError> {
        let _guard = self.lock();
        let Some(mut job) = self.get(job_id)? else {
            return Err(DomainError::NotFound(format!("job {job_id}")));
        };
        if job.status != JobStatus::Running || job.leased_by.as_deref() != Some(worker_id) {
            return Ok(());
        }
        let now = Utc::now();
        let attempt_no = job.attempts;
        job.status = JobStatus::Succeeded;
        job.result = result;
        job.error = None;
        job.leased_by = None;
        job.lease_expires_at = None;
        job.finished_at = Some(now);

        let mut batch = WriteBatch::default();
        self.stage_put_job(&mut batch, &job)?;
        self.stage_finalize_attempt(
            &mut batch,
            job_id,
            attempt_no,
            JobStatus::Succeeded,
            None,
            now,
        )?;
        self.db.write(batch)?;
        Ok(())
    }

    pub fn mark_failed(
        &self,
        job_id: JobId,
        worker_id: &str,
        error: &str,
    ) -> Result<JobStatus, DomainError> {
        let _guard = self.lock();
        let Some(mut job) = self.get(job_id)? else {
            return Err(DomainError::NotFound(format!("job {job_id}")));
        };
        if job.status != JobStatus::Running || job.leased_by.as_deref() != Some(worker_id) {
            return Ok(job.status);
        }
        let now = Utc::now();
        let attempt_no = job.attempts;
        let mut batch = WriteBatch::default();

        let (next_status, attempt_status) = if job.attempts >= job.max_attempts {
            job.status = JobStatus::Dead;
            job.leased_by = None;
            job.lease_expires_at = None;
            job.error = Some(error.to_owned());
            job.finished_at = Some(now);
            (JobStatus::Dead, JobStatus::Dead)
        } else {
            job.status = JobStatus::Queued;
            job.leased_by = None;
            job.lease_expires_at = None;
            job.error = Some(error.to_owned());
            job.available_at = now + chrono::Duration::seconds(backoff_seconds(job.attempts));
            self.stage_add_ready(&mut batch, &job);
            (JobStatus::Queued, JobStatus::Failed)
        };

        self.stage_put_job(&mut batch, &job)?;
        self.stage_finalize_attempt(
            &mut batch,
            job_id,
            attempt_no,
            attempt_status,
            Some(error),
            now,
        )?;
        self.db.write(batch)?;
        Ok(next_status)
    }

    pub fn reap_expired_leases(&self) -> Result<u64, DomainError> {
        let _guard = self.lock();
        let now = Utc::now();
        let mut expired = Vec::new();
        self.db.for_each("job", |_, blob| {
            let job: SbolJob = decode_job(blob)?;
            if job.status == JobStatus::Running && job.lease_expires_at.is_some_and(|t| t < now) {
                expired.push(job);
            }
            Ok(true)
        })?;

        let count = expired.len() as u64;
        let mut batch = WriteBatch::default();
        for mut job in expired {
            job.status = JobStatus::Queued;
            job.leased_by = None;
            job.lease_expires_at = None;
            job.available_at = now;
            if job.error.is_none() {
                job.error = Some("lease expired".to_owned());
            }
            self.stage_add_ready(&mut batch, &job);
            self.stage_put_job(&mut batch, &job)?;
        }
        self.db.write(batch)?;
        Ok(count)
    }

    pub fn cancel(&self, id: JobId) -> Result<bool, DomainError> {
        let _guard = self.lock();
        let Some(mut job) = self.get(id)? else {
            return Ok(false);
        };
        if !matches!(job.status, JobStatus::Queued | JobStatus::Running) {
            return Ok(false);
        }
        let now = Utc::now();
        let mut batch = WriteBatch::default();
        if job.status == JobStatus::Queued {
            self.stage_remove_ready(&mut batch, &job);
        }
        job.status = JobStatus::Cancelled;
        job.leased_by = None;
        job.lease_expires_at = None;
        if job.finished_at.is_none() {
            job.finished_at = Some(now);
        }
        self.stage_put_job(&mut batch, &job)?;
        self.db.write(batch)?;
        Ok(true)
    }

    // --- logs --------------------------------------------------------------

    pub fn append_log(
        &self,
        job_id: JobId,
        attempt_no: Option<i32>,
        level: &str,
        message: &str,
        fields: Value,
    ) -> Result<JobLogRecord, DomainError> {
        let _guard = self.lock();
        let log_id = self.next_counter("next_log_id")?;
        let record = JobLogRecord {
            id: log_id,
            job_id,
            attempt_no,
            level: normalize_log_level(level).to_owned(),
            message: message.to_owned(),
            fields,
            created_at: Utc::now(),
        };
        self.db
            .put_cf("job_log", &log_key(job_id, log_id), &encode(&record)?)?;
        Ok(record)
    }

    pub fn list_logs(
        &self,
        id: JobId,
        after_id: Option<i64>,
        limit: u32,
    ) -> Result<Vec<JobLogRecord>, DomainError> {
        let after = after_id.unwrap_or(0);
        let limit = limit.clamp(1, LOG_LIMIT_MAX) as usize;
        let prefix = id.0.as_bytes().to_vec();
        let mut out = Vec::new();
        self.db.for_each_prefix("job_log", &prefix, |_, blob| {
            let record: JobLogRecord = decode(blob)?;
            if record.id > after {
                out.push(record);
            }
            Ok(out.len() < limit)
        })?;
        Ok(out)
    }

    // --- reads -------------------------------------------------------------

    pub fn get(&self, id: JobId) -> Result<Option<SbolJob>, DomainError> {
        match self.db.get_cf("job", id.0.as_bytes())? {
            Some(blob) => Ok(Some(decode_job(&blob)?)),
            None => Ok(None),
        }
    }

    pub fn current_status(&self, id: JobId) -> Result<Option<JobStatus>, DomainError> {
        Ok(self.get(id)?.map(|j| j.status))
    }

    pub fn list_attempts(&self, id: JobId) -> Result<Vec<JobAttempt>, DomainError> {
        let prefix = id.0.as_bytes().to_vec();
        let mut out = Vec::new();
        self.db.for_each_prefix("job_attempt", &prefix, |_, blob| {
            out.push(decode::<JobAttempt>(blob)?);
            Ok(true)
        })?;
        out.sort_by_key(|r| std::cmp::Reverse(r.attempt_no));
        Ok(out)
    }

    pub fn list(&self, filter: &ListJobsFilter) -> Result<Vec<SbolJob>, DomainError> {
        let limit = filter.limit.clamp(1, LIST_LIMIT_MAX) as usize;
        let mut out = Vec::new();
        self.db.for_each("job", |_, blob| {
            let job: SbolJob = decode_job(blob)?;
            if filter.kind.as_ref().is_some_and(|k| &job.kind != k) {
                return Ok(true);
            }
            if filter.status.is_some_and(|s| job.status != s) {
                return Ok(true);
            }
            if filter.queue.as_ref().is_some_and(|q| &job.queue != q) {
                return Ok(true);
            }
            if filter
                .correlation_id
                .is_some_and(|c| job.correlation_id != Some(c))
            {
                return Ok(true);
            }
            if filter.since.is_some_and(|s| job.created_at < s) {
                return Ok(true);
            }
            out.push(job);
            Ok(true)
        })?;
        out.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.0.cmp(&a.id.0))
        });
        out.truncate(limit);
        Ok(out)
    }

    pub fn queue_depth_snapshot(&self) -> Result<Vec<QueueDepthRow>, DomainError> {
        use std::collections::BTreeMap;
        let mut counts: BTreeMap<(String, String), i64> = BTreeMap::new();
        self.db.for_each("job", |_, blob| {
            let job: SbolJob = decode_job(blob)?;
            if matches!(
                job.status,
                JobStatus::Queued | JobStatus::Running | JobStatus::Failed | JobStatus::Dead
            ) {
                *counts
                    .entry((job.status.as_db_str().to_owned(), job.queue.clone()))
                    .or_insert(0) += 1;
            }
            Ok(true)
        })?;
        let mut out = Vec::new();
        for ((status, queue), count) in counts {
            out.push(QueueDepthRow {
                status: JobStatus::from_db_str(&status)?,
                queue,
                count,
            });
        }
        Ok(out)
    }

    pub fn oldest_queued_age(&self) -> Result<Vec<OldestQueuedAge>, DomainError> {
        use std::collections::BTreeMap;
        let now = Utc::now();
        let mut oldest: BTreeMap<String, DateTime<Utc>> = BTreeMap::new();
        self.db.for_each("job", |_, blob| {
            let job: SbolJob = decode_job(blob)?;
            if job.status == JobStatus::Queued && job.available_at <= now {
                oldest
                    .entry(job.queue.clone())
                    .and_modify(|t| {
                        if job.available_at < *t {
                            *t = job.available_at;
                        }
                    })
                    .or_insert(job.available_at);
            }
            Ok(true)
        })?;
        Ok(oldest
            .into_iter()
            .map(|(queue, t)| OldestQueuedAge {
                queue,
                age_secs: (now - t).num_milliseconds() as f64 / 1000.0,
            })
            .collect())
    }

    // --- staging helpers ---------------------------------------------------

    fn stage_put_job(&self, batch: &mut WriteBatch, job: &SbolJob) -> Result<(), DomainError> {
        batch.put_cf(&self.db.cf("job"), job.id.0.as_bytes(), encode(job)?);
        Ok(())
    }

    fn stage_add_ready(&self, batch: &mut WriteBatch, job: &SbolJob) {
        batch.put_cf(&self.db.cf("job_ready"), ready_key(job), []);
    }

    fn stage_remove_ready(&self, batch: &mut WriteBatch, job: &SbolJob) {
        batch.delete_cf(&self.db.cf("job_ready"), ready_key(job));
    }

    fn stage_record_attempt_started(
        &self,
        batch: &mut WriteBatch,
        job: &SbolJob,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), DomainError> {
        let key = attempt_key(job.id, job.attempts);
        // INSERT OR IGNORE: a re-dequeue at the same attempt keeps the original.
        if self.db.exists_cf("job_attempt", &key)? {
            return Ok(());
        }
        let attempt = JobAttempt {
            id: self.next_counter("next_attempt_id")?,
            job_id: job.id,
            attempt_no: job.attempts,
            worker_id: worker_id.to_owned(),
            started_at: now,
            finished_at: None,
            status: JobStatus::Running,
            error: None,
        };
        batch.put_cf(&self.db.cf("job_attempt"), key, encode(&attempt)?);
        Ok(())
    }

    fn stage_finalize_attempt(
        &self,
        batch: &mut WriteBatch,
        job_id: JobId,
        attempt_no: i32,
        status: JobStatus,
        error: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<(), DomainError> {
        let key = attempt_key(job_id, attempt_no);
        if let Some(blob) = self.db.get_cf("job_attempt", &key)? {
            let mut attempt: JobAttempt = decode(&blob)?;
            attempt.finished_at = Some(now);
            attempt.status = status;
            attempt.error = error.map(|e| e.to_owned());
            batch.put_cf(&self.db.cf("job_attempt"), key, encode(&attempt)?);
        }
        Ok(())
    }

    fn next_counter(&self, name: &str) -> Result<i64, DomainError> {
        let current = match self.db.get_cf("meta", name.as_bytes())? {
            Some(bytes) if bytes.len() == 8 => {
                i64::from_be_bytes(bytes.try_into().expect("8 bytes"))
            }
            _ => 1,
        };
        self.db
            .put_cf("meta", name.as_bytes(), &(current + 1).to_be_bytes())?;
        Ok(current)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.write_lock.lock().unwrap_or_else(|e| e.into_inner())
    }
}

// --- key + value codecs ----------------------------------------------------

fn ready_key(job: &SbolJob) -> Vec<u8> {
    let mut key = job.queue.as_bytes().to_vec();
    key.push(crate::db::SEP);
    key.extend_from_slice(&encode_priority(job.priority));
    key.extend_from_slice(&encode_avail(job.available_at.timestamp_millis()));
    key.extend_from_slice(job.id.0.as_bytes());
    key
}

/// Higher priority sorts first: offset-binary, then inverted.
fn encode_priority(priority: i16) -> [u8; 2] {
    let offset = (priority as u16) ^ 0x8000;
    (0xFFFFu16 - offset).to_be_bytes()
}

/// Earlier `available_at` sorts first: offset-binary big-endian millis.
fn encode_avail(millis: i64) -> [u8; 8] {
    ((millis as u64) ^ 0x8000_0000_0000_0000).to_be_bytes()
}

fn decode_avail(bytes: &[u8]) -> i64 {
    let u = u64::from_be_bytes(bytes.try_into().expect("8 bytes"));
    (u ^ 0x8000_0000_0000_0000) as i64
}

fn attempt_key(job_id: JobId, attempt_no: i32) -> Vec<u8> {
    let mut key = job_id.0.as_bytes().to_vec();
    key.extend_from_slice(&attempt_no.to_be_bytes());
    key
}

fn log_key(job_id: JobId, log_id: i64) -> Vec<u8> {
    let mut key = job_id.0.as_bytes().to_vec();
    key.extend_from_slice(&log_id.to_be_bytes());
    key
}

fn idem_key(kind: &str, key: &str) -> Vec<u8> {
    compose(&[kind.as_bytes(), key.as_bytes()])
}

fn job_id_from_bytes(bytes: &[u8]) -> Result<JobId, DomainError> {
    let uuid = Uuid::from_slice(bytes).map_err(|_| DomainError::Database("bad job id".into()))?;
    Ok(JobId(uuid))
}

fn lease_duration(lease: Duration) -> chrono::Duration {
    chrono::Duration::from_std(lease).unwrap_or_else(|_| chrono::Duration::seconds(60))
}

/// Exponential backoff: 60s doubling per attempt, capped at one hour.
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

fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, DomainError> {
    serde_json::to_vec(value).map_err(|e| DomainError::Serialization(e.to_string()))
}

fn decode<T: for<'de> serde::Deserialize<'de>>(blob: &[u8]) -> Result<T, DomainError> {
    serde_json::from_slice(blob).map_err(|e| DomainError::Serialization(e.to_string()))
}

fn decode_job(blob: &[u8]) -> Result<SbolJob, DomainError> {
    decode(blob)
}
