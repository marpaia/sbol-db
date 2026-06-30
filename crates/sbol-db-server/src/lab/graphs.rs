//! `/lab/api/graphs` — paginated listing of every named graph.
//!
//! This is the graph-native browser backing the lab UI. Unlike the older
//! document listing (which only saw `kind = 'sbol3'` imports), this lists
//! **all** graphs, including `kind = 'verbatim'` graphs written through the
//! SynBioHub-compatible Graph Store / SPARQL Update endpoints. Each row carries
//! the graph's `kind`, its triple count, and (for graphs with a derived SBOL
//! view) its object count. `GET /lab/api/graphs/:id/triples` serves one graph's
//! raw triples, paginated — the way a `verbatim` graph (which has no derived
//! object view) is browsed in the UI.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use sbol_db_core::{GraphId, ObjectTerm, SubjectTerm, Triple};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ApiError;
use crate::AppState;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 500;

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
    /// Optional filter on graph kind (`sbol3` or `verbatim`).
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Serialize)]
pub struct GraphSummary {
    pub id: Uuid,
    pub iri: String,
    pub kind: String,
    pub name: Option<String>,
    pub serialization_format: Option<String>,
    pub source_uri: Option<String>,
    pub created_at: DateTime<Utc>,
    pub object_count: i64,
    pub triple_count: i64,
}

#[derive(Serialize)]
pub struct ListResponse {
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
    pub graphs: Vec<GraphSummary>,
}

const DEFAULT_TRIPLE_LIMIT: i64 = 100;
const MAX_TRIPLE_LIMIT: i64 = 1000;

#[derive(Deserialize)]
pub struct TriplesQuery {
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

/// One RDF term, shaped like a SPARQL-results JSON binding so the UI renders
/// it the same way it renders query output: `type` is `uri`, `bnode`, or
/// `literal`; `datatype` / `language` are populated for typed / tagged
/// literals.
#[derive(Serialize)]
pub struct Term {
    #[serde(rename = "type")]
    pub term_type: &'static str,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datatype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

impl Term {
    fn uri(value: String) -> Self {
        Term {
            term_type: "uri",
            value,
            datatype: None,
            language: None,
        }
    }
    fn bnode(value: String) -> Self {
        Term {
            term_type: "bnode",
            value,
            datatype: None,
            language: None,
        }
    }
    fn literal(value: String, datatype: Option<String>, language: Option<String>) -> Self {
        Term {
            term_type: "literal",
            value,
            datatype,
            language,
        }
    }
}

#[derive(Serialize)]
pub struct TripleRow {
    pub subject: Term,
    pub predicate: Term,
    pub object: Term,
}

#[derive(Serialize)]
pub struct TriplesResponse {
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
    pub triples: Vec<TripleRow>,
}

pub async fn list_graphs(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<ListResponse>, ApiError> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = q.offset.unwrap_or(0).max(0);
    let kind = q.kind.as_deref();

    let total = state.lab.count_graphs(kind).await?;
    let graphs = state
        .lab
        .list_graph_overviews(kind, limit, offset)
        .await?
        .into_iter()
        .map(overview_to_summary)
        .collect();

    Ok(Json(ListResponse {
        total,
        limit,
        offset,
        graphs,
    }))
}

pub async fn get_graph_detail(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<GraphSummary>, ApiError> {
    match state.lab.get_graph_overview(GraphId(id)).await? {
        Some(g) => Ok(Json(overview_to_summary(g))),
        None => Err(ApiError::NotFound(format!("graph {id}"))),
    }
}

/// Paginated raw triples of one graph. This is how a `verbatim` graph's
/// content is browsed in the UI — it has no derived object view, so the
/// triples themselves are the content. Works for any graph kind.
pub async fn get_graph_triples(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<TriplesQuery>,
) -> Result<Json<TriplesResponse>, ApiError> {
    let limit = q
        .limit
        .unwrap_or(DEFAULT_TRIPLE_LIMIT)
        .clamp(1, MAX_TRIPLE_LIMIT);
    let offset = q.offset.unwrap_or(0).max(0);

    match state.lab.graph_triples(GraphId(id), limit, offset).await? {
        None => Err(ApiError::NotFound(format!("graph {id}"))),
        Some(page) => {
            let triples = page.triples.into_iter().map(triple_to_row).collect();
            Ok(Json(TriplesResponse {
                total: page.total,
                limit,
                offset,
                triples,
            }))
        }
    }
}

fn overview_to_summary(g: sbol_db_storage::GraphOverview) -> GraphSummary {
    GraphSummary {
        id: g.id.0,
        iri: g.iri,
        kind: g.kind,
        name: g.name,
        serialization_format: g.serialization_format,
        source_uri: g.source_uri,
        created_at: g.created_at,
        object_count: g.object_count,
        triple_count: g.triple_count,
    }
}

fn triple_to_row(triple: Triple) -> TripleRow {
    let subject = match triple.subject {
        SubjectTerm::Iri(iri) => Term::uri(iri.into_inner()),
        SubjectTerm::BlankNode(node) => Term::bnode(node),
    };
    let object = match triple.object {
        ObjectTerm::Iri(iri) => Term::uri(iri.into_inner()),
        ObjectTerm::BlankNode(node) => Term::bnode(node),
        ObjectTerm::Literal {
            value,
            datatype,
            language,
        } => Term::literal(value, Some(datatype.into_inner()), language),
    };
    TripleRow {
        subject,
        predicate: Term::uri(triple.predicate.into_inner()),
        object,
    }
}
