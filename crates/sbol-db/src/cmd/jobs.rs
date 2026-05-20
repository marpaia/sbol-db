//! `sbol-db jobs` — operator surface for the async job queue.

use anyhow::{anyhow, Context, Result};
use sbol_db_core::JobId;
use sbol_db_jobs::default_registry;
use sbol_db_postgres::{EnqueueOutcome, JobRepository, JobStatus, ListJobsFilter, NewJob, PgPool};

use crate::cli::JobsAction;
use crate::output::print_json;

pub async fn run(pool: PgPool, action: JobsAction) -> Result<()> {
    match action {
        JobsAction::Enqueue {
            kind,
            payload,
            queue,
            priority,
            max_attempts,
            idempotency_key,
        } => {
            let payload = read_payload(&payload)?;
            let repo = JobRepository::new(pool);
            let outcome = repo
                .enqueue(NewJob {
                    kind,
                    payload,
                    queue,
                    priority,
                    max_attempts,
                    idempotency_key,
                    available_at: None,
                    parent_job_id: None,
                    correlation_id: None,
                })
                .await?;
            let (dedup, job) = match outcome {
                EnqueueOutcome::Inserted(j) => (false, j),
                EnqueueOutcome::AlreadyExists(j) => (true, j),
            };
            print_json(&serde_json::json!({
                "deduplicated": dedup,
                "job": job,
            }))
        }
        JobsAction::Status { id } => {
            let repo = JobRepository::new(pool);
            let job = repo
                .get(JobId(id))
                .await?
                .ok_or_else(|| anyhow!("no job with id {id}"))?;
            print_json(&job)
        }
        JobsAction::List {
            kind,
            status,
            queue,
            limit,
        } => {
            let status = match status.as_deref() {
                None => None,
                Some(s) => Some(parse_cli_job_status(s)?),
            };
            let repo = JobRepository::new(pool);
            let jobs = repo
                .list(&ListJobsFilter {
                    kind,
                    status,
                    queue,
                    correlation_id: None,
                    since: None,
                    limit,
                })
                .await?;
            print_json(&jobs)
        }
        JobsAction::Cancel { id } => {
            let repo = JobRepository::new(pool);
            let cancelled = repo.cancel(JobId(id)).await?;
            print_json(&serde_json::json!({ "cancelled": cancelled }))
        }
        JobsAction::Attempts { id } => {
            let repo = JobRepository::new(pool);
            let attempts = repo.list_attempts(JobId(id)).await?;
            print_json(&attempts)
        }
        JobsAction::Replay {
            id,
            keep_idempotency_key,
        } => {
            let repo = JobRepository::new(pool);
            let original = repo
                .get(JobId(id))
                .await?
                .ok_or_else(|| anyhow!("no job with id {id}"))?;
            // Refuse if the handler vanished — otherwise the replay would
            // sit queued forever.
            let registry = default_registry();
            if registry.lookup(&original.kind).is_none() {
                return Err(anyhow!(
                    "no registered handler for kind {} — cannot replay",
                    original.kind
                ));
            }
            let outcome = repo
                .enqueue(NewJob {
                    kind: original.kind.clone(),
                    payload: original.payload.clone(),
                    queue: Some(original.queue.clone()),
                    priority: Some(original.priority),
                    max_attempts: Some(original.max_attempts),
                    idempotency_key: if keep_idempotency_key {
                        original.idempotency_key.clone()
                    } else {
                        None
                    },
                    available_at: None,
                    parent_job_id: original.parent_job_id,
                    correlation_id: original.correlation_id,
                })
                .await?;
            let (dedup, job) = match outcome {
                EnqueueOutcome::Inserted(j) => (false, j),
                EnqueueOutcome::AlreadyExists(j) => (true, j),
            };
            print_json(&serde_json::json!({
                "replayed_from": id,
                "deduplicated": dedup,
                "job": job,
            }))
        }
        JobsAction::QueueDepth => {
            let repo = JobRepository::new(pool);
            let rows = repo.queue_depth_snapshot().await?;
            let payload: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|r| {
                    serde_json::json!({
                        "status": r.status,
                        "queue": r.queue,
                        "count": r.count,
                    })
                })
                .collect();
            print_json(&payload)
        }
        JobsAction::QueueAge => {
            let repo = JobRepository::new(pool);
            let rows = repo.oldest_queued_age().await?;
            let payload: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|r| {
                    serde_json::json!({
                        "queue": r.queue,
                        "age_secs": r.age_secs,
                    })
                })
                .collect();
            print_json(&payload)
        }
        JobsAction::Handlers => {
            let registry = default_registry();
            let kinds: Vec<&'static str> = registry.kinds().collect();
            print_json(&serde_json::json!({
                "kinds": kinds,
                "count": kinds.len(),
            }))
        }
    }
}

fn parse_cli_job_status(s: &str) -> Result<JobStatus> {
    Ok(match s {
        "queued" => JobStatus::Queued,
        "running" => JobStatus::Running,
        "succeeded" => JobStatus::Succeeded,
        "failed" => JobStatus::Failed,
        "cancelled" => JobStatus::Cancelled,
        "dead" => JobStatus::Dead,
        other => return Err(anyhow!("unknown job status: {other}")),
    })
}

/// Read the `payload` argument: inline JSON or `@path` for file-backed JSON.
fn read_payload(spec: &str) -> Result<serde_json::Value> {
    let body = if let Some(path) = spec.strip_prefix('@') {
        std::fs::read_to_string(path).with_context(|| format!("reading payload from {path}"))?
    } else {
        spec.to_owned()
    };
    serde_json::from_str(&body).with_context(|| "payload is not valid JSON")
}
