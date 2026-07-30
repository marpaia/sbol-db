//! `update_vector_index` job handler.
//!
//! The payload names documents whose current primary-store state should be
//! synchronized into one active vector generation. Existing objects are
//! projected and embedded as upserts; absent objects become idempotent
//! deletes. This makes retry behavior independent of the mutation event that
//! originally queued the job.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use sbol_db_core::{GraphId, IriString, SbolObjectRecord};
use sbol_db_search::{VectorDocumentChange, VectorUpdateSpec};
use sbol_db_search_sdk::DocumentId;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::rebuild_vector_index::project_object;
use crate::context::JobContext;
use crate::handler::{HandlerError, JobHandler, JobOutcome};

pub const KIND: &str = "update_vector_index";
/// Internal desired-state job used by the application scheduler. Unlike
/// [`KIND`], it deliberately does not name a generation: the handler resolves
/// the active generation when it runs, so a queued event follows a successful
/// concurrent rebuild instead of becoming permanently stale.
pub const MAINTAIN_KIND: &str = "maintain_vector_index";
const LOOKUP_BATCH_SIZE: usize = 1_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateVectorIndexPayload {
    pub artifact_id: String,
    pub generation: String,
    pub document_ids: Vec<String>,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

const fn default_batch_size() -> usize {
    64
}

/// Payload for the internally scheduled active-generation maintenance job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaintainVectorIndexPayload {
    pub artifact_id: String,
    pub document_ids: Vec<String>,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

/// Synchronize selected SBOL object identities into a configured vector
/// index. The target generation must still be active when the operation starts
/// and completes.
pub struct UpdateVectorIndexHandler;

#[async_trait]
impl JobHandler for UpdateVectorIndexHandler {
    type Payload = UpdateVectorIndexPayload;

    fn kind(&self) -> &'static str {
        KIND
    }

    async fn run(
        &self,
        ctx: JobContext,
        payload: UpdateVectorIndexPayload,
    ) -> Result<JobOutcome, HandlerError> {
        if payload.artifact_id.trim().is_empty() || payload.generation.trim().is_empty() {
            return Err(HandlerError::InvalidPayload(
                "artifact_id and generation cannot be empty".to_owned(),
            ));
        }
        if payload.batch_size == 0 {
            return Err(HandlerError::InvalidPayload(
                "batch_size must be greater than zero".to_owned(),
            ));
        }
        let maintainers = ctx.vector_indexes.as_ref().ok_or_else(|| {
            HandlerError::Other(
                "update_vector_index requires vector index maintainers on the job context; \
                 this worker was built without one"
                    .to_owned(),
            )
        })?;
        let maintainer = maintainers.get(&payload.artifact_id).ok_or_else(|| {
            HandlerError::Other(format!(
                "update_vector_index has no maintainer for artifact {:?}; configured indexes: {:?}",
                payload.artifact_id,
                maintainers.indexes()
            ))
        })?;
        let document_ids = validate_document_ids(payload.document_ids)?;
        ensure_not_cancelled(&ctx)?;
        ctx.log(
            "info",
            "incremental vector index update started",
            json!({
                "artifact_id": payload.artifact_id,
                "generation": payload.generation,
                "documents": document_ids.len(),
            }),
        )
        .await;

        let changes = project_current_state(&ctx, &document_ids).await?;
        ensure_not_cancelled(&ctx)?;
        let report = maintainer
            .update(
                VectorUpdateSpec {
                    artifact_id: payload.artifact_id,
                    generation: payload.generation,
                    batch_size: payload.batch_size,
                },
                changes,
            )
            .await
            .map_err(|error| {
                HandlerError::Other(format!("updating vector index incrementally: {error}"))
            })?;

        let result = serde_json::to_value(&report)?;
        ctx.log(
            "info",
            "incremental vector index update completed",
            result.clone(),
        )
        .await;
        Ok(JobOutcome::with_result(result))
    }
}

/// Synchronize selected desired-state documents into whichever generation is
/// active when the job executes. This is the safe hand-off from application
/// mutation hooks to a vector backend: the existing explicit-generation job
/// remains available for operators that intentionally need stale-generation
/// detection.
pub struct MaintainVectorIndexHandler;

#[async_trait]
impl JobHandler for MaintainVectorIndexHandler {
    type Payload = MaintainVectorIndexPayload;

    fn kind(&self) -> &'static str {
        MAINTAIN_KIND
    }

    async fn run(
        &self,
        ctx: JobContext,
        payload: MaintainVectorIndexPayload,
    ) -> Result<JobOutcome, HandlerError> {
        if payload.artifact_id.trim().is_empty() {
            return Err(HandlerError::InvalidPayload(
                "artifact_id cannot be empty".to_owned(),
            ));
        }
        if payload.batch_size == 0 {
            return Err(HandlerError::InvalidPayload(
                "batch_size must be greater than zero".to_owned(),
            ));
        }
        let maintainers = ctx.vector_indexes.as_ref().ok_or_else(|| {
            HandlerError::Other(
                "maintain_vector_index requires vector index maintainers on the job context; \
                 this worker was built without one"
                    .to_owned(),
            )
        })?;
        let maintainer = maintainers.get(&payload.artifact_id).ok_or_else(|| {
            HandlerError::Other(format!(
                "maintain_vector_index has no maintainer for artifact {:?}; configured indexes: {:?}",
                payload.artifact_id,
                maintainers.indexes()
            ))
        })?;
        let document_ids = validate_document_ids(payload.document_ids)?;
        ensure_not_cancelled(&ctx)?;
        let generation = maintainer
            .active_generation(&payload.artifact_id)
            .await
            .map_err(|error| {
                HandlerError::Other(format!(
                    "resolving active vector generation for maintenance: {error}"
                ))
            })?
            .ok_or_else(|| {
                HandlerError::Other(format!(
                    "maintain_vector_index has no active generation for artifact {:?}",
                    payload.artifact_id
                ))
            })?;
        ctx.log(
            "info",
            "scheduled vector index maintenance started",
            json!({
                "artifact_id": payload.artifact_id,
                "generation": generation.handle.generation,
                "documents": document_ids.len(),
            }),
        )
        .await;

        let changes = project_current_state(&ctx, &document_ids).await?;
        ensure_not_cancelled(&ctx)?;
        let report = maintainer
            .update(
                VectorUpdateSpec {
                    artifact_id: payload.artifact_id,
                    generation: generation.handle.generation,
                    batch_size: payload.batch_size,
                },
                changes,
            )
            .await
            .map_err(|error| {
                HandlerError::Other(format!("maintaining vector index incrementally: {error}"))
            })?;

        let result = serde_json::to_value(&report)?;
        ctx.log(
            "info",
            "scheduled vector index maintenance completed",
            result.clone(),
        )
        .await;
        Ok(JobOutcome::with_result(result))
    }
}

fn validate_document_ids(document_ids: Vec<String>) -> Result<Vec<String>, HandlerError> {
    if document_ids.is_empty() {
        return Err(HandlerError::InvalidPayload(
            "document_ids must contain at least one SBOL object IRI".to_owned(),
        ));
    }
    let mut seen = HashSet::with_capacity(document_ids.len());
    document_ids
        .into_iter()
        .map(|document_id| {
            let document_id = IriString::new(document_id)
                .map_err(|error| HandlerError::InvalidPayload(error.to_string()))?
                .into_inner();
            if !seen.insert(document_id.clone()) {
                return Err(HandlerError::InvalidPayload(format!(
                    "duplicate document id {document_id:?}"
                )));
            }
            Ok(document_id)
        })
        .collect()
}

async fn project_current_state(
    ctx: &JobContext,
    document_ids: &[String],
) -> Result<Vec<VectorDocumentChange>, HandlerError> {
    let mut changes = Vec::with_capacity(document_ids.len());
    let mut graph_iris: HashMap<GraphId, Option<String>> = HashMap::new();

    for ids in document_ids.chunks(LOOKUP_BATCH_SIZE) {
        ensure_not_cancelled(ctx)?;
        let records = ctx
            .service
            .get_objects_by_iris(&ids.iter().map(String::as_str).collect::<Vec<_>>())
            .await?;
        let mut records = records
            .into_iter()
            .map(|record| (record.iri.as_str().to_owned(), record))
            .collect::<HashMap<String, SbolObjectRecord>>();

        for document_id in ids {
            ensure_not_cancelled(ctx)?;
            let Some(record) = records.remove(document_id) else {
                changes.push(VectorDocumentChange::Delete {
                    document_id: DocumentId(document_id.clone()),
                });
                continue;
            };
            let graph = match record.graph_id {
                None => None,
                Some(graph_id) => match graph_iris.get(&graph_id) {
                    Some(graph) => graph.clone(),
                    None => {
                        let graph = ctx
                            .service
                            .get_graph(graph_id)
                            .await?
                            .and_then(|record| record.document_iri)
                            .map(|iri| iri.into_inner());
                        graph_iris.insert(graph_id, graph.clone());
                        graph
                    }
                },
            };
            changes.push(VectorDocumentChange::Upsert(project_object(record, graph)));
        }
    }

    Ok(changes)
}

fn ensure_not_cancelled(ctx: &JobContext) -> Result<(), HandlerError> {
    if ctx.is_cancelled() {
        return Err(HandlerError::Other(
            "incremental vector index update cancelled".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_ids_are_validated_and_deduplicated() {
        assert!(validate_document_ids(Vec::new()).is_err());
        assert!(validate_document_ids(vec!["not an iri".to_owned()]).is_err());
        assert!(validate_document_ids(vec![
            "https://example.org/a".to_owned(),
            "https://example.org/a".to_owned(),
        ])
        .is_err());
        assert_eq!(
            validate_document_ids(vec!["https://example.org/a".to_owned()]).unwrap(),
            ["https://example.org/a"]
        );
    }
}
