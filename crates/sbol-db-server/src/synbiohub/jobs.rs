//! SynBioHub v1 job routes: the caller's job list, the admin job list, and the
//! cancel/restart actions. Classic renders these as pages; the adapter answers
//! them as JSON over the native job queue.

use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use sbol_db_core::JobId;
use sbol_db_storage::{ListJobsFilter, NewJob, SbolJob};
use serde::Deserialize;
use serde_json::{json, Value};

use super::auth::parse_body;
use super::CurrentUser;
use crate::error::ApiError;
use crate::AppState;

/// The default page size for a job listing.
const JOB_LIST_LIMIT: u32 = 100;

/// Render a job as the JSON row classic's job views project.
fn job_json(job: &SbolJob) -> Value {
    json!({
        "id": job.id.to_string(),
        "kind": job.kind,
        "status": format!("{:?}", job.status).to_lowercase(),
        "queue": job.queue,
        "attempts": job.attempts,
        "maxAttempts": job.max_attempts,
        "error": job.error,
        "createdAt": job.created_at.to_rfc3339(),
        "startedAt": job.started_at.map(|t| t.to_rfc3339()),
        "finishedAt": job.finished_at.map(|t| t.to_rfc3339()),
    })
}

async fn list_recent(state: &AppState) -> Result<Vec<Value>, ApiError> {
    let filter = ListJobsFilter {
        kind: None,
        status: None,
        queue: None,
        correlation_id: None,
        since: None,
        limit: JOB_LIST_LIMIT,
    };
    let jobs = state.jobs.list(&filter).await?;
    Ok(jobs.iter().map(job_json).collect())
}

/// `GET /admin/jobs` — every job. The admin gate is applied by the admin router.
pub async fn admin_jobs(State(state): State<AppState>) -> Result<Response, ApiError> {
    Ok(Json(list_recent(&state).await?).into_response())
}

#[derive(Debug, Deserialize)]
struct JobAction {
    #[serde(alias = "jobId", alias = "job")]
    id: JobId,
}

/// `POST /actions/job/cancel` — cancel a queued or running job by id.
pub async fn cancel_job(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    if user.is_none() {
        return Ok(unauthorized());
    }
    let action: JobAction = parse_body(&headers, &body)?;
    let cancelled = state.jobs.cancel(action.id).await?;
    Ok(Json(json!({ "cancelled": cancelled })).into_response())
}

/// `POST /actions/job/restart` — re-enqueue a finished or failed job with the
/// same kind and payload.
pub async fn restart_job(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    if user.is_none() {
        return Ok(unauthorized());
    }
    let action: JobAction = parse_body(&headers, &body)?;
    let Some(job) = state.jobs.get(action.id).await? else {
        return Err(ApiError::NotFound(format!("job {}", action.id)));
    };
    let outcome = state
        .jobs
        .enqueue(NewJob {
            kind: job.kind,
            payload: job.payload,
            queue: Some(job.queue),
            priority: Some(job.priority),
            max_attempts: Some(job.max_attempts),
            idempotency_key: None,
            available_at: None,
            parent_job_id: None,
            correlation_id: None,
        })
        .await?;
    let id = match outcome {
        sbol_db_storage::EnqueueOutcome::Inserted(job)
        | sbol_db_storage::EnqueueOutcome::AlreadyExists(job) => job.id,
    };
    Ok(Json(json!({ "jobId": id.to_string() })).into_response())
}

/// `GET /admin/virtuoso` — classic reports its Virtuoso datastore status here.
/// sbol-db has no Virtuoso; it reports the native store instead.
pub async fn virtuoso() -> Response {
    Json(json!({ "engine": "native", "virtuoso": false })).into_response()
}

/// `GET /admin/listLogs` — classic lists its on-disk log files. sbol-db logs to
/// the process, so it reports the durable job-log stream is the log surface.
pub async fn list_logs() -> Response {
    Json(json!({ "logs": [], "source": "job-log" })).into_response()
}

/// `GET /corruptLog` — classic's corrupted-object log. sbol-db validates on
/// submit, so nothing is stored corrupt; the log is empty.
pub async fn corrupt_log() -> Response {
    Json(json!({ "entries": [] })).into_response()
}

fn unauthorized() -> Response {
    (
        axum::http::StatusCode::UNAUTHORIZED,
        [(CONTENT_TYPE, "text/plain")],
        "authentication required",
    )
        .into_response()
}
