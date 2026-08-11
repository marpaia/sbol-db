//! Universal RDF catalog endpoints for the administrator UI.

use std::collections::HashSet;

use axum::extract::{Path, Query, State};
use axum::Json;
use sbol_db_core::{GraphId, ObjectTerm, SubjectTerm, Triple};
use sbol_db_storage::{NamedGraphQuery, ResourceQuery, SequenceQuery, TriplePageQuery};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::super::error::V2Error;
use crate::error::ApiError;
use crate::AppState;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct PageQuery {
    after: Option<String>,
    limit: Option<u32>,
    q: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ResourcePageQuery {
    after: Option<String>,
    limit: Option<u32>,
    q: Option<String>,
    class: Option<String>,
    role: Option<String>,
    graph: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResourceLookupQuery {
    iri: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResourceLookupRequest {
    iris: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct TripleQuery {
    after: Option<String>,
    limit: Option<u32>,
}

pub(super) async fn dashboard(State(state): State<AppState>) -> Result<Json<Value>, V2Error> {
    let recent_graphs = NamedGraphQuery {
        limit: 5,
        ..NamedGraphQuery::default()
    };
    let (stats, graphs, top_classes, ontologies) = tokio::try_join!(
        state.app.store.catalog_stats(),
        state.app.store.catalog_graphs(&recent_graphs),
        state.app.store.catalog_top_classes(10),
        state.app.store.list_ontologies(),
    )?;
    Ok(Json(json!({
        "counts": stats,
        "graphs": graphs.items,
        "top_classes": top_classes,
        "loaded_ontologies": ontologies,
    })))
}

pub(super) async fn list_graphs(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Value>, V2Error> {
    let page = state
        .app
        .store
        .catalog_graphs(&NamedGraphQuery {
            after: query.after,
            limit: query.limit.unwrap_or(50),
            text: normalized(query.q),
        })
        .await?;
    Ok(Json(
        json!({ "items": page.items, "next_cursor": page.next_cursor }),
    ))
}

pub(super) async fn get_graph(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, V2Error> {
    let graph = state
        .app
        .store
        .catalog_graph(GraphId(id))
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("graph {id}")))?;
    Ok(Json(json!(graph)))
}

pub(super) async fn graph_triples(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<TripleQuery>,
) -> Result<Json<Value>, V2Error> {
    let page = state
        .app
        .store
        .catalog_graph_triples(
            GraphId(id),
            &TriplePageQuery {
                after: query.after,
                limit: query.limit.unwrap_or(100),
            },
        )
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("graph {id}")))?;
    let items: Vec<TripleRow> = page.items.into_iter().map(Into::into).collect();
    Ok(Json(
        json!({ "items": items, "next_cursor": page.next_cursor }),
    ))
}

pub(super) async fn list_resources(
    State(state): State<AppState>,
    Query(query): Query<ResourcePageQuery>,
) -> Result<Json<Value>, V2Error> {
    let page = state
        .app
        .store
        .catalog_resources(&ResourceQuery {
            after: query.after,
            limit: query.limit.unwrap_or(50),
            text: normalized(query.q),
            class: normalized(query.class),
            role: normalized(query.role),
            graph_iri: normalized(query.graph),
        })
        .await?;
    Ok(Json(
        json!({ "items": page.items, "next_cursor": page.next_cursor }),
    ))
}

pub(super) async fn get_resource(
    State(state): State<AppState>,
    Query(query): Query<ResourceLookupQuery>,
) -> Result<Json<Value>, V2Error> {
    let resource = state
        .app
        .store
        .catalog_resource(&query.iri)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("resource {}", query.iri)))?;
    let occurrences = state
        .app
        .store
        .catalog_resource_occurrences(&query.iri)
        .await?;
    Ok(Json(
        json!({ "resource": resource, "occurrences": occurrences }),
    ))
}

pub(super) async fn lookup_resources(
    State(state): State<AppState>,
    Json(request): Json<ResourceLookupRequest>,
) -> Result<Json<Value>, V2Error> {
    if request.iris.len() > 1_000 {
        return Err(
            ApiError::BadRequest("resource lookup accepts at most 1000 IRIs".to_owned()).into(),
        );
    }
    let iris: Vec<String> = request
        .iris
        .into_iter()
        .map(|iri| iri.trim().to_owned())
        .filter(|iri| !iri.is_empty())
        .collect();
    let found = state.app.store.catalog_resources_by_iris(&iris).await?;
    let found_iris: HashSet<&str> = found.iter().map(|resource| resource.iri.as_str()).collect();
    let missing: Vec<String> = iris
        .into_iter()
        .filter(|iri| !found_iris.contains(iri.as_str()))
        .collect();
    Ok(Json(json!({ "found": found, "missing": missing })))
}

pub(super) async fn list_sequences(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Value>, V2Error> {
    let page = state
        .app
        .store
        .catalog_sequences(&SequenceQuery {
            after: query.after,
            limit: query.limit.unwrap_or(50),
            text: normalized(query.q),
        })
        .await?;
    Ok(Json(
        json!({ "items": page.items, "next_cursor": page.next_cursor }),
    ))
}

fn normalized(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

#[derive(Serialize)]
struct Term {
    #[serde(rename = "type")]
    kind: &'static str,
    value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    datatype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
}

#[derive(Serialize)]
struct TripleRow {
    subject: Term,
    predicate: Term,
    object: Term,
}

impl From<Triple> for TripleRow {
    fn from(triple: Triple) -> Self {
        Self {
            subject: match triple.subject {
                SubjectTerm::Iri(value) => iri(value.into_inner()),
                SubjectTerm::BlankNode(value) => blank(value),
            },
            predicate: iri(triple.predicate.into_inner()),
            object: match triple.object {
                ObjectTerm::Iri(value) => iri(value.into_inner()),
                ObjectTerm::BlankNode(value) => blank(value),
                ObjectTerm::Literal {
                    value,
                    datatype,
                    language,
                } => Term {
                    kind: "literal",
                    value,
                    datatype: Some(datatype.into_inner()),
                    language,
                },
            },
        }
    }
}

fn iri(value: String) -> Term {
    Term {
        kind: "uri",
        value,
        datatype: None,
        language: None,
    }
}

fn blank(value: String) -> Term {
    Term {
        kind: "bnode",
        value,
        datatype: None,
        language: None,
    }
}
