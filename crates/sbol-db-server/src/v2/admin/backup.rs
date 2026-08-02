//! Administrator control plane for the one complete-backup strategy.
//!
//! This endpoint never assembles an archive in the request task. Manual UI
//! requests and deployment gates enqueue the same durable `complete_backup`
//! job that the scheduler uses, then expose its normal job lifecycle/result.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use sbol_db_app::AdminAuditOutcome;
use sbol_db_jobs::{BackupTrigger, CompleteBackupPayload, COMPLETE_BACKUP_JOB_KIND};
use sbol_db_storage::{EnqueueOutcome, ListJobsFilter, NewJob};
use serde::Deserialize;
use serde_json::{json, Value};

use super::super::auth::Identity;
use super::super::error::V2Error;
use super::super::util::parse_json;
use crate::error::ApiError;
use crate::AppState;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TriggerRequest {
    trigger: Option<BackupTrigger>,
    idempotency_key: Option<String>,
}

pub(super) async fn status(State(state): State<AppState>) -> Result<Json<Value>, V2Error> {
    let recent = state
        .jobs
        .list(&ListJobsFilter {
            kind: Some(COMPLETE_BACKUP_JOB_KIND.to_owned()),
            limit: 25,
            ..ListJobsFilter::default()
        })
        .await?;
    Ok(Json(json!({
        "enabled": state.config.complete_backups_enabled,
        "strategy": "complete_encrypted_checkpoint",
        "components": ["rocksdb", "blobs", "search", "acme"],
        "recent": recent,
    })))
}

pub(super) async fn trigger(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), V2Error> {
    if !state.config.complete_backups_enabled {
        return Err(ApiError::Unavailable(
            "complete backups require the self-contained production RocksDB runtime".to_owned(),
        )
        .into());
    }
    let request = if body.is_empty() {
        TriggerRequest::default()
    } else {
        parse_json(&body)?
    };
    let trigger = request.trigger.unwrap_or(BackupTrigger::Manual);
    if trigger == BackupTrigger::Scheduled {
        return Err(ApiError::BadRequest(
            "the scheduled trigger is reserved for the internal scheduler".to_owned(),
        )
        .into());
    }
    let idempotency_key = normalize_idempotency_key(trigger, request.idempotency_key)?;
    let actor = identity
        .0
        .as_ref()
        .expect("admin middleware attaches an administrator")
        .username
        .clone();
    let audit = state.app.admin_audit_service();
    audit
        .record(
            "backup.create",
            &actor,
            trigger_label(trigger),
            AdminAuditOutcome::Attempted,
            None,
        )
        .await?;

    let mut input = NewJob::new(
        COMPLETE_BACKUP_JOB_KIND,
        serde_json::to_value(CompleteBackupPayload::new(trigger, Some(actor.clone())))
            .map_err(|error| ApiError::BadRequest(error.to_string()))?,
    );
    input.priority = Some(100);
    input.max_attempts = Some(3);
    input.idempotency_key = idempotency_key;
    let outcome = match state.jobs.enqueue(input).await {
        Ok(outcome) => outcome,
        Err(error) => {
            audit
                .record(
                    "backup.create",
                    &actor,
                    trigger_label(trigger),
                    AdminAuditOutcome::Failed,
                    Some(&error.to_string()),
                )
                .await?;
            return Err(error.into());
        }
    };
    let (job, deduplicated) = match outcome {
        EnqueueOutcome::Inserted(job) => (job, false),
        EnqueueOutcome::AlreadyExists(job) => (job, true),
    };
    audit
        .record(
            "backup.create",
            &actor,
            &job.id.to_string(),
            AdminAuditOutcome::Succeeded,
            Some(&format!("trigger={}", trigger_label(trigger))),
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "job": job, "deduplicated": deduplicated })),
    ))
}

fn normalize_idempotency_key(
    trigger: BackupTrigger,
    value: Option<String>,
) -> Result<Option<String>, V2Error> {
    let value = value.map(|value| value.trim().to_owned());
    if trigger == BackupTrigger::PreDeploy && value.as_deref().is_none_or(str::is_empty) {
        return Err(ApiError::BadRequest(
            "pre_deploy backups require an idempotency_key such as the release SHA".to_owned(),
        )
        .into());
    }
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.len() > 200 || value.chars().any(char::is_control) {
        return Err(ApiError::BadRequest(
            "idempotency_key must be at most 200 printable characters".to_owned(),
        )
        .into());
    }
    Ok(Some(format!(
        "complete-backup:{}:{value}",
        trigger_label(trigger)
    )))
}

fn trigger_label(trigger: BackupTrigger) -> &'static str {
    match trigger {
        BackupTrigger::Manual => "manual",
        BackupTrigger::Scheduled => "scheduled",
        BackupTrigger::PreDeploy => "pre_deploy",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predeploy_requires_a_stable_idempotency_key() {
        assert!(normalize_idempotency_key(BackupTrigger::PreDeploy, None).is_err());
        assert_eq!(
            normalize_idempotency_key(BackupTrigger::PreDeploy, Some(" release-abc ".to_owned()))
                .unwrap(),
            Some("complete-backup:pre_deploy:release-abc".to_owned())
        );
    }

    #[test]
    fn manual_requests_can_be_distinct_or_idempotent() {
        assert_eq!(
            normalize_idempotency_key(BackupTrigger::Manual, None).unwrap(),
            None
        );
        assert_eq!(
            normalize_idempotency_key(BackupTrigger::Manual, Some("click-1".to_owned())).unwrap(),
            Some("complete-backup:manual:click-1".to_owned())
        );
    }
}
