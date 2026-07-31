//! Privileged operational commands and their read-only status projections.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use sbol_db_app::AdminAuditOutcome;
use sbol_db_core::JobId;
use sbol_db_storage::{
    EnqueueOutcome, JobAttempt, JobLogRecord, JobStatus, ListJobsFilter, NewJob,
    OntologyLoadReport, OntologyRecord, SbolJob,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::super::auth::Identity;
use super::super::error::V2Error;
use super::super::util::parse_json;
use super::confirmation;
use crate::error::ApiError;
use crate::AppState;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct ListJobsQuery {
    kind: Option<String>,
    status: Option<String>,
    queue: Option<String>,
    correlation_id: Option<String>,
    limit: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct EnqueueRequest {
    kind: String,
    payload: Value,
    queue: Option<String>,
    priority: Option<i16>,
    max_attempts: Option<i32>,
    idempotency_key: Option<String>,
    correlation_id: Option<Uuid>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct LogsQuery {
    after_id: Option<i64>,
    limit: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Confirmation {
    confirmation: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct OntologyRequest {
    prefix: String,
    url: Option<String>,
    name: Option<String>,
}

pub(super) async fn list_jobs(
    State(state): State<AppState>,
    Query(query): Query<ListJobsQuery>,
) -> Result<Json<Vec<SbolJob>>, V2Error> {
    let status = query.status.as_deref().map(parse_status).transpose()?;
    let correlation_id = query
        .correlation_id
        .as_deref()
        .map(parse_uuid)
        .transpose()?;
    let limit = query
        .limit
        .as_deref()
        .map(|value| {
            value
                .parse::<u32>()
                .map_err(|_| ApiError::BadRequest("limit must be an integer".to_owned()))
        })
        .transpose()?
        .unwrap_or(100)
        .clamp(1, 1_000);
    Ok(Json(
        state
            .jobs
            .list(&ListJobsFilter {
                kind: query.kind,
                status,
                queue: query.queue,
                correlation_id,
                since: None,
                limit,
            })
            .await?,
    ))
}

pub(super) async fn enqueue_job(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    body: Bytes,
) -> Result<Json<Value>, V2Error> {
    let request: EnqueueRequest = parse_json(&body)?;
    if request.kind.trim().is_empty() {
        return Err(ApiError::BadRequest("kind is required".to_owned()).into());
    }
    record(
        &state,
        &identity,
        "job.enqueue",
        &request.kind,
        AdminAuditOutcome::Attempted,
        None,
    )
    .await?;
    let outcome = state
        .jobs
        .enqueue(NewJob {
            kind: request.kind.clone(),
            payload: request.payload,
            queue: request.queue,
            priority: request.priority,
            max_attempts: request.max_attempts,
            idempotency_key: request.idempotency_key,
            available_at: None,
            parent_job_id: None,
            correlation_id: request.correlation_id,
        })
        .await?;
    let (job, deduplicated) = match outcome {
        EnqueueOutcome::Inserted(job) => (job, false),
        EnqueueOutcome::AlreadyExists(job) => (job, true),
    };
    record(
        &state,
        &identity,
        "job.enqueue",
        &job.id.to_string(),
        AdminAuditOutcome::Succeeded,
        Some(&format!("kind={}", request.kind)),
    )
    .await?;
    Ok(Json(json!({ "job": job, "deduplicated": deduplicated })))
}

pub(super) async fn get_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SbolJob>, V2Error> {
    let id = job_id(&id)?;
    Ok(Json(
        state
            .jobs
            .get(id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("job {id}")))?,
    ))
}

pub(super) async fn job_attempts(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<JobAttempt>>, V2Error> {
    let id = job_id(&id)?;
    state
        .jobs
        .get(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("job {id}")))?;
    Ok(Json(state.jobs.list_attempts(id).await?))
}

pub(super) async fn job_logs(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<LogsQuery>,
) -> Result<Json<Vec<JobLogRecord>>, V2Error> {
    let id = job_id(&id)?;
    state
        .jobs
        .get(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("job {id}")))?;
    Ok(Json(
        state
            .jobs
            .list_logs(
                id,
                query.after_id,
                query.limit.unwrap_or(500).clamp(1, 2_000),
            )
            .await?,
    ))
}

pub(super) async fn cancel_job(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Value>, V2Error> {
    let request: Confirmation = parse_json(&body)?;
    confirmation(&request.confirmation, &format!("CANCEL JOB {id}"))?;
    let job_id = job_id(&id)?;
    state
        .jobs
        .get(job_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("job {id}")))?;
    record(
        &state,
        &identity,
        "job.cancel",
        &id,
        AdminAuditOutcome::Attempted,
        None,
    )
    .await?;
    let cancelled = state.jobs.cancel(job_id).await?;
    record(
        &state,
        &identity,
        "job.cancel",
        &id,
        AdminAuditOutcome::Succeeded,
        Some(if cancelled {
            "job cancelled"
        } else {
            "job was already terminal"
        }),
    )
    .await?;
    Ok(Json(json!({ "cancelled": cancelled })))
}

pub(super) async fn list_ontologies(
    State(state): State<AppState>,
) -> Result<Json<Vec<OntologyRecord>>, V2Error> {
    Ok(Json(state.service.list_ontologies().await?))
}

pub(super) async fn load_ontology(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    body: Bytes,
) -> Result<Json<OntologyLoadReport>, V2Error> {
    let request: OntologyRequest = parse_json(&body)?;
    let prefix = request.prefix.trim().to_ascii_uppercase();
    if prefix.is_empty() {
        return Err(ApiError::BadRequest("prefix is required".to_owned()).into());
    }
    let (url, name) = ontology_defaults(&prefix, request.url, request.name)?;
    record(
        &state,
        &identity,
        "ontology.load",
        &prefix,
        AdminAuditOutcome::Attempted,
        None,
    )
    .await?;
    let report = state
        .service
        .load_ontology_from_url(&prefix, &name, &url)
        .await?;
    state.invalidate_lab_caches();
    record(
        &state,
        &identity,
        "ontology.load",
        &prefix,
        AdminAuditOutcome::Succeeded,
        None,
    )
    .await?;
    Ok(Json(report))
}

pub(super) async fn search_status(State(state): State<AppState>) -> Result<Json<Value>, V2Error> {
    let runtime = state.app.search_runtime();
    let recent_rebuilds = state
        .jobs
        .list(&ListJobsFilter {
            kind: Some("rebuild_search_index".to_owned()),
            limit: 10,
            ..ListJobsFilter::default()
        })
        .await?;
    Ok(Json(json!({
        "default_strategy": runtime.default_strategy(),
        "strategies": runtime.descriptors(),
        "recent_rebuilds": recent_rebuilds,
    })))
}

pub(super) async fn rebuild_search(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<Value>, V2Error> {
    record(
        &state,
        &identity,
        "search.rebuild",
        "all-indexes",
        AdminAuditOutcome::Attempted,
        None,
    )
    .await?;
    let outcome = state
        .jobs
        .enqueue(NewJob::new("rebuild_search_index", json!({})))
        .await?;
    let (job, deduplicated) = match outcome {
        EnqueueOutcome::Inserted(job) => (job, false),
        EnqueueOutcome::AlreadyExists(job) => (job, true),
    };
    record(
        &state,
        &identity,
        "search.rebuild",
        &job.id.to_string(),
        AdminAuditOutcome::Succeeded,
        None,
    )
    .await?;
    Ok(Json(json!({ "job": job, "deduplicated": deduplicated })))
}

async fn record(
    state: &AppState,
    identity: &Identity,
    action: &str,
    target: &str,
    outcome: AdminAuditOutcome,
    detail: Option<&str>,
) -> Result<(), V2Error> {
    let actor = identity
        .0
        .as_ref()
        .map(|user| user.username.as_str())
        .unwrap_or("unknown");
    state
        .app
        .admin_audit_service()
        .record(action, actor, target, outcome, detail)
        .await?;
    Ok(())
}

fn parse_status(value: &str) -> Result<JobStatus, V2Error> {
    JobStatus::from_db_str(value).map_err(Into::into)
}

fn parse_uuid(value: &str) -> Result<Uuid, V2Error> {
    Uuid::parse_str(value)
        .map_err(|_| ApiError::BadRequest("correlation_id must be a UUID".to_owned()).into())
}

fn job_id(value: &str) -> Result<JobId, V2Error> {
    Ok(JobId(Uuid::parse_str(value).map_err(|_| {
        ApiError::BadRequest("job id must be a UUID".to_owned())
    })?))
}

fn ontology_defaults(
    prefix: &str,
    url: Option<String>,
    name: Option<String>,
) -> Result<(String, String), V2Error> {
    match prefix.to_ascii_lowercase().as_str() {
        "so" => Ok((
            url.unwrap_or_else(|| "http://purl.obolibrary.org/obo/so.obo".to_owned()),
            name.unwrap_or_else(|| "Sequence Ontology".to_owned()),
        )),
        "sbo" => Ok((
            url.unwrap_or_else(|| "http://purl.obolibrary.org/obo/sbo.obo".to_owned()),
            name.unwrap_or_else(|| "Systems Biology Ontology".to_owned()),
        )),
        _ => Ok((
            url.filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    ApiError::BadRequest(format!("url is required for ontology prefix {prefix}"))
                })?,
            name.unwrap_or_else(|| prefix.to_owned()),
        )),
    }
}
