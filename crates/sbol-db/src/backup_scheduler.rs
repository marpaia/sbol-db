//! Idempotent wall-clock scheduling for the one complete-backup job.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use sbol_db_core::JobId;
use sbol_db_jobs::{BackupTrigger, CompleteBackupPayload, COMPLETE_BACKUP_JOB_KIND};
use sbol_db_storage::{EnqueueOutcome, JobQueue, NewJob};
use tokio_util::sync::CancellationToken;

const MIN_INTERVAL_SECS: u64 = 15 * 60;
const MAX_INTERVAL_SECS: u64 = 30 * 24 * 60 * 60;
const ERROR_RETRY_SECS: u64 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScheduleSlot {
    number: i64,
    next_start_unix: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScheduledEnqueue {
    job_id: JobId,
    deduplicated: bool,
}

pub fn validate_interval(interval: Duration) -> Result<()> {
    let seconds = interval.as_secs();
    if !(MIN_INTERVAL_SECS..=MAX_INTERVAL_SECS).contains(&seconds) {
        bail!(
            "backup interval must be between {MIN_INTERVAL_SECS} and {MAX_INTERVAL_SECS} seconds"
        );
    }
    Ok(())
}

pub async fn run(
    jobs: Arc<dyn JobQueue>,
    interval: Duration,
    cancel: CancellationToken,
) -> Result<()> {
    validate_interval(interval)?;
    metrics::gauge!("sbol_db_backup_scheduler_interval_seconds").set(interval.as_secs_f64());
    loop {
        let now = Utc::now();
        let slot = schedule_slot(now, interval);
        let retry_after_error = match enqueue(jobs.as_ref(), interval, now).await {
            Ok(outcome) => {
                metrics::counter!(
                    "sbol_db_backup_scheduler_enqueues_total",
                    "result" => if outcome.deduplicated { "deduplicated" } else { "inserted" },
                )
                .increment(1);
                metrics::gauge!("sbol_db_backup_scheduler_last_enqueue_timestamp_seconds")
                    .set(now.timestamp() as f64);
                tracing::info!(
                    job_id = %outcome.job_id,
                    deduplicated = outcome.deduplicated,
                    slot = slot.number,
                    "scheduled complete backup"
                );
                false
            }
            Err(error) => {
                metrics::counter!(
                    "sbol_db_backup_scheduler_enqueues_total",
                    "result" => "error",
                )
                .increment(1);
                tracing::error!(error = %error, slot = slot.number, "schedule complete backup");
                true
            }
        };

        let wait = if retry_after_error {
            Duration::from_secs(ERROR_RETRY_SECS.min(interval.as_secs()))
        } else {
            metrics::gauge!("sbol_db_backup_scheduler_next_timestamp_seconds")
                .set(slot.next_start_unix as f64);
            Duration::from_secs(
                slot.next_start_unix
                    .saturating_sub(Utc::now().timestamp())
                    .max(1) as u64,
            )
        };
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

async fn enqueue(
    jobs: &dyn JobQueue,
    interval: Duration,
    now: DateTime<Utc>,
) -> Result<ScheduledEnqueue> {
    let slot = schedule_slot(now, interval);
    let mut input = NewJob::new(
        COMPLETE_BACKUP_JOB_KIND,
        serde_json::to_value(CompleteBackupPayload {
            trigger: BackupTrigger::Scheduled,
            requested_at: now,
            requested_by: Some("internal-scheduler".to_owned()),
        })
        .context("encode scheduled backup payload")?,
    );
    input.priority = Some(50);
    input.max_attempts = Some(3);
    input.idempotency_key = Some(format!(
        "complete-backup:scheduled:{}:{}",
        interval.as_secs(),
        slot.number
    ));
    let (job, deduplicated) = match jobs.enqueue(input).await? {
        EnqueueOutcome::Inserted(job) => (job, false),
        EnqueueOutcome::AlreadyExists(job) => (job, true),
    };
    Ok(ScheduledEnqueue {
        job_id: job.id,
        deduplicated,
    })
}

fn schedule_slot(now: DateTime<Utc>, interval: Duration) -> ScheduleSlot {
    let seconds = interval.as_secs() as i64;
    let number = now.timestamp().div_euclid(seconds);
    ScheduleSlot {
        number,
        next_start_unix: number.saturating_add(1).saturating_mul(seconds),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use sbol_db_rocksdb::{Db, RocksdbJobs};

    #[test]
    fn slots_are_stable_wall_clock_buckets() {
        let interval = Duration::from_secs(3_600);
        let before = Utc.with_ymd_and_hms(2026, 8, 1, 12, 59, 59).unwrap();
        let after = Utc.with_ymd_and_hms(2026, 8, 1, 13, 0, 0).unwrap();
        let first = schedule_slot(before, interval);
        let second = schedule_slot(after, interval);
        assert_eq!(first.next_start_unix, after.timestamp());
        assert_eq!(second.number, first.number + 1);
    }

    #[tokio::test]
    async fn duplicate_enqueues_reuse_the_same_durable_job() {
        let temp = tempfile::tempdir().unwrap();
        let jobs = RocksdbJobs::new(Db::open(temp.path()).unwrap());
        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 30, 0).unwrap();
        let interval = Duration::from_secs(3_600);

        let first = enqueue(&jobs, interval, now).await.unwrap();
        let second = enqueue(&jobs, interval, now).await.unwrap();

        assert!(!first.deduplicated);
        assert!(second.deduplicated);
        assert_eq!(first.job_id, second.job_id);
    }
}
