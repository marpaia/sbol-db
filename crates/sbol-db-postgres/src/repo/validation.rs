use sbol::ValidationReport;
use sbol_db_core::{DocumentId, DomainError, Severity, ValidationRunId, ValidationStatus};
use sqlx::Row;
use uuid::Uuid;

use crate::repo::db_err;
use crate::PgPool;

#[derive(Clone)]
pub struct ValidationRepository {
    _pool: PgPool,
}

#[derive(Debug, Clone)]
pub struct RecordedValidation {
    pub run_id: ValidationRunId,
    pub status: ValidationStatus,
    pub issue_count: usize,
}

impl ValidationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { _pool: pool }
    }

    /// Record a validation run plus all of its findings. Run inside the
    /// caller's transaction.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_run(
        &self,
        conn: &mut sqlx::PgConnection,
        target_iri: &str,
        target_document_id: Option<DocumentId>,
        validator_name: &str,
        validator_version: Option<&str>,
        ruleset: &str,
        report: &ValidationReport,
    ) -> Result<RecordedValidation, DomainError> {
        let status = classify(report);
        let run_row = sqlx::query(
            r#"
            INSERT INTO sbol_validation_runs (
                target_iri, target_document_id, validator_name, validator_version,
                ruleset, status, finished_at, summary
            ) VALUES ($1, $2, $3, $4, $5, $6, now(), $7)
            RETURNING id
            "#,
        )
        .bind(target_iri)
        .bind(target_document_id.map(|d| d.0))
        .bind(validator_name)
        .bind(validator_version)
        .bind(ruleset)
        .bind(status.as_db_str())
        .bind(serde_json::json!({
            "issue_count": report.issues().len(),
            "error_count": report.errors().count(),
            "warning_count": report.warnings().count(),
        }))
        .fetch_one(&mut *conn)
        .await
        .map_err(db_err)?;
        let run_id: Uuid = run_row.try_get("id").map_err(db_err)?;

        for issue in report.issues() {
            let severity = match issue.severity {
                sbol::Severity::Warning => Severity::Warning,
                sbol::Severity::Error => Severity::Error,
                _ => Severity::Warning,
            };
            let subject_iri = issue.subject.as_iri().map(|i| i.as_str().to_owned());
            sqlx::query(
                r#"
                INSERT INTO sbol_validation_findings (
                    validation_run_id, severity, rule_id, message, subject_iri, path
                ) VALUES ($1, $2, $3, $4, $5, $6)
                "#,
            )
            .bind(run_id)
            .bind(severity.as_db_str())
            .bind(issue.rule)
            .bind(&issue.message)
            .bind(subject_iri)
            .bind(issue.property)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
        }

        Ok(RecordedValidation {
            run_id: ValidationRunId(run_id),
            status,
            issue_count: report.issues().len(),
        })
    }
}

fn classify(report: &ValidationReport) -> ValidationStatus {
    if report.has_errors() {
        ValidationStatus::Failed
    } else if report.warnings().next().is_some() {
        ValidationStatus::Warning
    } else {
        ValidationStatus::Passed
    }
}
