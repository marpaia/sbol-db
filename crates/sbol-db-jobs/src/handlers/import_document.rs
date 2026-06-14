//! `import_document` job handler.
//!
//! Wraps [`SbolObjectService::import_document`] in a job: payload is the
//! inline RDF body plus metadata; `result` is the serialised
//! [`sbol_db_core::ImportReport`]. The synchronous `POST /documents`
//! endpoint stays the right surface for small one-shot imports; this
//! handler is the right surface for queued / fanned-out corpus work
//! where you want a job id to poll, retries on transient DB failures,
//! and visibility into per-file progress across a cluster of workers.

use async_trait::async_trait;
use sbol_db_core::{IriString, SerializationFormat};
use sbol_db_postgres::ImportInput;
use serde::{Deserialize, Serialize};

use crate::context::JobContext;
use crate::handler::{HandlerError, JobHandler, JobOutcome};

pub const KIND: &str = "import_document";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportDocumentPayload {
    pub body: String,
    pub format: SerializationFormat,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub source_uri: Option<String>,
    #[serde(default)]
    pub document_iri: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub created_by: Option<String>,
}

pub struct ImportDocumentHandler;

#[async_trait]
impl JobHandler for ImportDocumentHandler {
    type Payload = ImportDocumentPayload;

    fn kind(&self) -> &'static str {
        KIND
    }

    async fn run(
        &self,
        ctx: JobContext,
        payload: Self::Payload,
    ) -> Result<JobOutcome, HandlerError> {
        let document_iri = payload
            .document_iri
            .map(IriString::new)
            .transpose()
            .map_err(|e| HandlerError::InvalidPayload(e.to_string()))?;
        ctx.log(
            "info",
            "document import starting",
            serde_json::json!({
                "format": payload.format,
                "bytes": payload.body.len(),
                "namespace": payload.namespace.as_deref(),
                "source_uri": payload.source_uri.as_deref(),
                "name": payload.name.as_deref(),
            }),
        )
        .await;
        let report = ctx
            .service
            .import_document(ImportInput {
                body: payload.body,
                format: payload.format,
                namespace: payload.namespace,
                source_uri: payload.source_uri,
                document_iri,
                created_by: payload.created_by,
                name: payload.name,
                description: payload.description,
            })
            .await?;
        ctx.log(
            "info",
            "document import completed",
            serde_json::json!({
                "document_id": report.document_id,
                "object_count": report.object_count,
                "quad_count": report.quad_count,
                "validation_status": report.validation_status,
                "validation_issue_count": report.validation_issue_count,
            }),
        )
        .await;
        let result = serde_json::to_value(&report)?;
        Ok(JobOutcome::with_result(result))
    }
}
