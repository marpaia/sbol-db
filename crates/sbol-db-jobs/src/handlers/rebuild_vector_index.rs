//! `rebuild_vector_index` job handler.
//!
//! Projects the backend-neutral derived SBOL object view into stable embedding
//! text and backend payloads, then delegates generation construction to
//! [`VectorIndexMaintainer`](sbol_db_search::VectorIndexMaintainer). The
//! maintainer owns the create/write/flush/optimize/activate protocol, so a
//! failed job cannot replace the active generation.

use std::collections::{BTreeMap, HashMap};

use async_trait::async_trait;
use sbol_db_core::{GraphId, SbolObjectRecord};
use sbol_db_search::{VectorDocument, VectorRebuildSpec};
use sbol_db_search_sdk::DocumentId;
use sbol_db_storage::ListObjectsFilter;
use serde_json::{json, Value};

use crate::context::JobContext;
use crate::handler::{HandlerError, JobHandler, JobOutcome};

pub const KIND: &str = "rebuild_vector_index";
const PAGE_SIZE: u32 = 5_000;

/// Rebuild the vector artifact described by [`VectorRebuildSpec`] from every
/// live record in the primary store's derived object view.
pub struct RebuildVectorIndexHandler;

#[async_trait]
impl JobHandler for RebuildVectorIndexHandler {
    type Payload = VectorRebuildSpec;

    fn kind(&self) -> &'static str {
        KIND
    }

    async fn run(
        &self,
        ctx: JobContext,
        payload: VectorRebuildSpec,
    ) -> Result<JobOutcome, HandlerError> {
        let maintainers = ctx.vector_indexes.as_ref().ok_or_else(|| {
            HandlerError::Other(
                "rebuild_vector_index requires vector index maintainers on the job context; \
                 this worker was built without one"
                    .to_owned(),
            )
        })?;
        let maintainer = maintainers.get(&payload.artifact_id).ok_or_else(|| {
            HandlerError::Other(format!(
                "rebuild_vector_index has no maintainer for artifact {:?}; configured indexes: {:?}",
                payload.artifact_id,
                maintainers.indexes()
            ))
        })?;
        ensure_not_cancelled(&ctx)?;
        ctx.log(
            "info",
            "vector index rebuild started",
            json!({
                "artifact_id": payload.artifact_id,
                "generation": payload.generation,
                "embedding_profile": payload.embedding_profile,
            }),
        )
        .await;

        let documents = project_corpus(&ctx).await?;
        ensure_not_cancelled(&ctx)?;
        let report = maintainer
            .rebuild(payload, documents)
            .await
            .map_err(|error| HandlerError::Other(format!("rebuilding vector index: {error}")))?;

        let result = serde_json::to_value(&report)?;
        ctx.log("info", "vector index rebuilt", result.clone())
            .await;
        Ok(JobOutcome::with_result(result))
    }
}

async fn project_corpus(ctx: &JobContext) -> Result<Vec<VectorDocument>, HandlerError> {
    let mut documents = Vec::new();
    let mut graph_iris: HashMap<GraphId, Option<String>> = HashMap::new();
    let mut after_iri = None;

    loop {
        ensure_not_cancelled(ctx)?;
        let page = ctx
            .service
            .list_objects(&ListObjectsFilter {
                after_iri: after_iri.clone(),
                limit: PAGE_SIZE,
                ..ListObjectsFilter::default()
            })
            .await?;
        if page.is_empty() {
            break;
        }
        after_iri = page.last().map(|record| record.iri.as_str().to_owned());
        let page_len = page.len();

        for record in page {
            ensure_not_cancelled(ctx)?;
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
            documents.push(project_object(record, graph));
        }

        if page_len < PAGE_SIZE as usize {
            break;
        }
    }

    Ok(documents)
}

/// The canonical embedding projection is intentionally small and labeled.
/// Derived JSON is excluded because its ordering and shape are parser/version
/// concerns; stable SBOL identity and human-facing metadata carry the initial
/// semantic-search contract.
fn project_object(record: SbolObjectRecord, graph: Option<String>) -> VectorDocument {
    let mut types = record.types;
    types.sort();
    types.dedup();
    let mut roles = record.roles;
    roles.sort();
    roles.dedup();

    let mut fields = vec![
        format!("URI: {}", record.iri),
        format!("SBOL class: {}", normalize_text(&record.sbol_class)),
    ];
    push_field(&mut fields, "Display ID", record.display_id.as_deref());
    push_field(&mut fields, "Name", record.name.as_deref());
    push_field(&mut fields, "Description", record.description.as_deref());
    for value in &types {
        push_field(&mut fields, "Type", Some(value));
    }
    for value in &roles {
        push_field(&mut fields, "Role", Some(value));
    }

    let mut payload = BTreeMap::from([("sbol_class".to_owned(), json!(record.sbol_class))]);
    if let Some(graph) = graph {
        payload.insert("graph".to_owned(), Value::String(graph));
    }

    VectorDocument {
        document_id: DocumentId(record.iri.into_inner()),
        text: fields.join("\n"),
        payload,
    }
}

fn push_field(fields: &mut Vec<String>, label: &str, value: Option<&str>) {
    let Some(value) = value.map(normalize_text).filter(|value| !value.is_empty()) else {
        return;
    };
    fields.push(format!("{label}: {value}"));
}

fn normalize_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn ensure_not_cancelled(ctx: &JobContext) -> Result<(), HandlerError> {
    if ctx.is_cancelled() {
        return Err(HandlerError::Other(
            "vector index rebuild cancelled".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use sbol_db_core::{IriString, ObjectId};

    use super::*;

    #[test]
    fn projection_is_stable_labeled_and_filterable() {
        let document = project_object(
            SbolObjectRecord {
                id: ObjectId::new(),
                iri: IriString::unchecked("https://example.org/component"),
                sbol_class: "Component".to_owned(),
                display_id: Some("promoter_1".to_owned()),
                name: Some("  Strong\n promoter  ".to_owned()),
                description: None,
                graph_id: None,
                types: vec!["DNA".to_owned(), "DNA".to_owned(), "Engineered".to_owned()],
                roles: vec!["promoter".to_owned(), "engineered".to_owned()],
                data: json!({"unstable": "excluded"}),
                content_hash: vec![],
            },
            Some("https://example.org/graph/public".to_owned()),
        );

        assert_eq!(document.document_id.0, "https://example.org/component");
        assert_eq!(
            document.text,
            "URI: https://example.org/component\n\
             SBOL class: Component\n\
             Display ID: promoter_1\n\
             Name: Strong promoter\n\
             Type: DNA\n\
             Type: Engineered\n\
             Role: engineered\n\
             Role: promoter"
        );
        assert_eq!(
            document.payload,
            BTreeMap::from([
                (
                    "graph".to_owned(),
                    json!("https://example.org/graph/public")
                ),
                ("sbol_class".to_owned(), json!("Component")),
            ])
        );
        assert!(!document.text.contains("unstable"));
    }
}
