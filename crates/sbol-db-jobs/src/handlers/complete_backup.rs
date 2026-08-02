//! One complete-backup job shared by manual, scheduled, and deployment gates.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{HandlerError, JobContext, JobHandler, JobOutcome};

pub const COMPLETE_BACKUP_JOB_KIND: &str = "complete_backup";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupTrigger {
    Manual,
    Scheduled,
    PreDeploy,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteBackupPayload {
    pub trigger: BackupTrigger,
    pub requested_at: DateTime<Utc>,
    pub requested_by: Option<String>,
}

impl CompleteBackupPayload {
    pub fn new(trigger: BackupTrigger, requested_by: Option<String>) -> Self {
        Self {
            trigger,
            requested_at: Utc::now(),
            requested_by,
        }
    }
}

pub struct CompleteBackupHandler;

#[async_trait]
impl JobHandler for CompleteBackupHandler {
    type Payload = CompleteBackupPayload;

    fn kind(&self) -> &'static str {
        COMPLETE_BACKUP_JOB_KIND
    }

    async fn run(
        &self,
        ctx: JobContext,
        payload: Self::Payload,
    ) -> Result<JobOutcome, HandlerError> {
        if ctx.is_cancelled() {
            return Err(HandlerError::Other(
                "backup cancelled before checkpoint creation".to_owned(),
            ));
        }
        let backups = ctx.backups.as_ref().ok_or_else(|| {
            HandlerError::Other(
                "complete backups are unavailable on this worker; run the embedded production worker"
                    .to_owned(),
            )
        })?;
        ctx.log(
            "info",
            "complete backup started",
            serde_json::json!({
                "trigger": payload.trigger,
                "requested_at": payload.requested_at,
                "requested_by": payload.requested_by,
            }),
        )
        .await;
        let started = std::time::Instant::now();
        let completed = match backups
            .create(ctx.job_id.as_uuid(), payload.requested_at)
            .await
        {
            Ok(completed) => completed,
            Err(error) => {
                let elapsed = started.elapsed().as_secs_f64();
                metrics::counter!(
                    "sbol_db_backups_completed_total",
                    "trigger" => trigger_label(payload.trigger),
                    "status" => "failed",
                )
                .increment(1);
                metrics::histogram!(
                    "sbol_db_backup_duration_seconds",
                    "trigger" => trigger_label(payload.trigger),
                )
                .record(elapsed);
                metrics::gauge!("sbol_db_backup_last_failure_timestamp_seconds")
                    .set(Utc::now().timestamp() as f64);
                return Err(HandlerError::Other(format!(
                    "complete backup failed: {error:#}"
                )));
            }
        };
        let created = &completed.local;
        let elapsed = started.elapsed().as_secs_f64();
        metrics::counter!(
            "sbol_db_backups_completed_total",
            "trigger" => trigger_label(payload.trigger),
            "status" => "verified",
        )
        .increment(1);
        metrics::histogram!(
            "sbol_db_backup_duration_seconds",
            "trigger" => trigger_label(payload.trigger),
        )
        .record(elapsed);
        metrics::gauge!("sbol_db_backup_last_success_timestamp_seconds")
            .set(Utc::now().timestamp() as f64);
        metrics::gauge!("sbol_db_backup_last_artifact_bytes").set(created.artifact_bytes as f64);
        if let Some(remote) = &completed.remote {
            metrics::gauge!("sbol_db_backup_last_remote_verification_timestamp_seconds")
                .set(remote.verified_at.timestamp() as f64);
        }
        ctx.log(
            "info",
            "complete backup verified and published",
            serde_json::json!({
                "backup_id": created.backup_id,
                "artifact_sha256": created.artifact_sha256,
                "artifact_bytes": created.artifact_bytes,
                "payload_bytes": created.payload_bytes,
                "files": created.files,
                "referenced_blobs": created.referenced_blobs,
                "reused": created.reused,
                "remote": &completed.remote,
                "disk_preflight": &completed.disk_preflight,
                "elapsed_secs": elapsed,
            }),
        )
        .await;
        let mut result = serde_json::to_value(&completed)?;
        if let Some(object) = result.as_object_mut() {
            object.insert("trigger".to_owned(), serde_json::to_value(payload.trigger)?);
            object.insert(
                "requested_by".to_owned(),
                serde_json::json!(payload.requested_by),
            );
        }
        Ok(JobOutcome::with_result(result))
    }
}

fn trigger_label(trigger: BackupTrigger) -> &'static str {
    match trigger {
        BackupTrigger::Manual => "manual",
        BackupTrigger::Scheduled => "scheduled",
        BackupTrigger::PreDeploy => "pre_deploy",
    }
}
