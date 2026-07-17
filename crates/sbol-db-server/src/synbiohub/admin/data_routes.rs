//! Admin data reads: every named graph, and an all-graph SPARQL endpoint.
//!
//! Both routes see the whole triplestore. `GET /admin/graphs` reuses the lab
//! graph-listing over [`LabStore`](sbol_db_storage::LabStore); `GET
//! /admin/sparql` runs through the same [`SparqlEngine`](sbol_db_sparql) as the
//! caller-scoped `/search`-family routes but with an unrestricted
//! [`GraphScope::Union`], so an administrator reads private user graphs the ACL
//! scope would otherwise hide.

use axum::extract::{Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::error::ApiError;
use crate::AppState;

/// Paging for `GET /admin/graphs`.
#[derive(Debug, Default, Deserialize)]
pub struct GraphsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// Optional filter on graph kind (`sbol3` or `verbatim`).
    pub kind: Option<String>,
}

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 500;

/// `GET /admin/graphs` — every named graph, paginated, with its kind, triple
/// count, and (for graphs with a derived SBOL view) object count.
#[cfg(feature = "lab")]
pub async fn graphs(
    State(state): State<AppState>,
    Query(q): Query<GraphsQuery>,
) -> Result<axum::Json<serde_json::Value>, ApiError> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = q.offset.unwrap_or(0).max(0);
    let kind = q.kind.as_deref();

    let total = state.lab.count_graphs(kind).await?;
    let graphs: Vec<serde_json::Value> = state
        .lab
        .list_graph_overviews(kind, limit, offset)
        .await?
        .into_iter()
        .map(|g| {
            serde_json::json!({
                "id": g.id.0,
                "iri": g.iri,
                "kind": g.kind,
                "name": g.name,
                "serializationFormat": g.serialization_format,
                "sourceUri": g.source_uri,
                "createdAt": g.created_at,
                "objectCount": g.object_count,
                "tripleCount": g.triple_count,
            })
        })
        .collect();

    Ok(axum::Json(serde_json::json!({
        "total": total,
        "limit": limit,
        "offset": offset,
        "graphs": graphs,
    })))
}

/// `GET /admin/graphs` — unavailable when the lab feature (which owns the graph
/// browser) is compiled out.
#[cfg(not(feature = "lab"))]
pub async fn graphs(State(_state): State<AppState>) -> Result<Response, ApiError> {
    Err(ApiError::Unavailable(
        "graph listing requires the lab feature".to_owned(),
    ))
}

/// The SPARQL query, taken from the `query` parameter as the SPARQL protocol
/// specifies for `GET`.
#[derive(Debug, Default, Deserialize)]
pub struct SparqlQuery {
    pub query: Option<String>,
}

/// `GET /admin/sparql?query=…` — run a read query against the whole
/// triplestore. Unlike the caller-scoped read routes, this passes
/// [`GraphScope::Union`], so the query sees every named graph.
pub async fn sparql(
    State(state): State<AppState>,
    Query(q): Query<SparqlQuery>,
) -> Result<Response, ApiError> {
    let query = q
        .query
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ApiError::BadRequest("a query parameter is required".to_owned()))?;

    let options = sbol_db_sparql::SparqlOptions {
        authorized_graphs: sbol_db_sparql::GraphScope::Union,
        ..Default::default()
    };
    let outcome = state.sparql.execute(&query, None, None, &options).await?;
    Ok((
        StatusCode::OK,
        [(CONTENT_TYPE, outcome.payload.content_type)],
        outcome.payload.body,
    )
        .into_response())
}
