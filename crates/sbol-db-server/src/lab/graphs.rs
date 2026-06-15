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
use serde::{Deserialize, Serialize};
use sqlx::Row;
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
    let pool = state.service.pool();

    let total: i64 = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM sbol_graphs WHERE ($1::text IS NULL OR kind = $1)",
    )
    .bind(q.kind.as_deref())
    .fetch_one(pool)
    .await
    .map_err(db)?;

    let rows = sqlx::query(
        r#"
        SELECT
          g.id,
          g.iri,
          g.kind,
          g.name,
          g.serialization_format,
          g.source_uri,
          g.created_at,
          coalesce(o.n, 0) AS object_count,
          coalesce(q.n, 0) AS triple_count
        FROM sbol_graphs g
        LEFT JOIN (
          SELECT graph_id, count(*) AS n
          FROM sbol_objects
          WHERE graph_id IS NOT NULL
          GROUP BY graph_id
        ) o ON o.graph_id = g.id
        LEFT JOIN (
          SELECT graph_iri, count(*) AS n
          FROM sbol_triples
          WHERE graph_iri IS NOT NULL
          GROUP BY graph_iri
        ) q ON q.graph_iri = g.iri
        WHERE ($1::text IS NULL OR g.kind = $1)
        ORDER BY g.created_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(q.kind.as_deref())
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(db)?;

    let graphs = rows
        .into_iter()
        .map(row_to_summary)
        .collect::<Result<Vec<_>, _>>()?;

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
    let pool = state.service.pool();
    let row = sqlx::query(
        r#"
        SELECT
          g.id,
          g.iri,
          g.kind,
          g.name,
          g.serialization_format,
          g.source_uri,
          g.created_at,
          (SELECT count(*) FROM sbol_objects WHERE graph_id = g.id) AS object_count,
          (SELECT count(*) FROM sbol_triples   WHERE graph_iri = g.iri) AS triple_count
        FROM sbol_graphs g
        WHERE g.id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db)?;

    match row {
        None => Err(ApiError::NotFound(format!("graph {id}"))),
        Some(row) => Ok(Json(row_to_summary(row)?)),
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
    let pool = state.service.pool();

    let iri: Option<String> =
        sqlx::query_scalar::<_, String>("SELECT iri FROM sbol_graphs WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(db)?;
    let iri = iri.ok_or_else(|| ApiError::NotFound(format!("graph {id}")))?;

    let total: i64 =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sbol_triples WHERE graph_iri = $1")
            .bind(&iri)
            .fetch_one(pool)
            .await
            .map_err(db)?;

    let rows = sqlx::query(
        r#"
        SELECT subject_iri, subject_blank, predicate_iri,
               object_iri, object_blank, object_literal, datatype_iri, language
        FROM sbol_triples
        WHERE graph_iri = $1
        ORDER BY subject_iri NULLS LAST, subject_blank NULLS LAST, predicate_iri, id
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(&iri)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(db)?;

    let triples = rows
        .into_iter()
        .map(row_to_triple_row)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Json(TriplesResponse {
        total,
        limit,
        offset,
        triples,
    }))
}

fn row_to_triple_row(row: sqlx::postgres::PgRow) -> Result<TripleRow, ApiError> {
    let subject_iri: Option<String> = row.try_get("subject_iri").map_err(db)?;
    let subject_blank: Option<String> = row.try_get("subject_blank").map_err(db)?;
    let predicate_iri: String = row.try_get("predicate_iri").map_err(db)?;
    let object_iri: Option<String> = row.try_get("object_iri").map_err(db)?;
    let object_blank: Option<String> = row.try_get("object_blank").map_err(db)?;
    let object_literal: Option<String> = row.try_get("object_literal").map_err(db)?;
    let datatype_iri: Option<String> = row.try_get("datatype_iri").map_err(db)?;
    let language: Option<String> = row.try_get("language").map_err(db)?;

    let subject = match (subject_iri, subject_blank) {
        (Some(v), _) => Term::uri(v),
        (None, Some(v)) => Term::bnode(v),
        (None, None) => Term::uri(String::new()),
    };
    let object = match (object_iri, object_blank, object_literal) {
        (Some(v), _, _) => Term::uri(v),
        (None, Some(v), _) => Term::bnode(v),
        (None, None, Some(v)) => Term::literal(v, datatype_iri, language),
        (None, None, None) => Term::literal(String::new(), None, None),
    };

    Ok(TripleRow {
        subject,
        predicate: Term::uri(predicate_iri),
        object,
    })
}

fn row_to_summary(row: sqlx::postgres::PgRow) -> Result<GraphSummary, ApiError> {
    Ok(GraphSummary {
        id: row.try_get("id").map_err(db)?,
        iri: row.try_get("iri").map_err(db)?,
        kind: row.try_get("kind").map_err(db)?,
        name: row.try_get("name").map_err(db)?,
        serialization_format: row.try_get("serialization_format").map_err(db)?,
        source_uri: row.try_get("source_uri").map_err(db)?,
        created_at: row.try_get("created_at").map_err(db)?,
        object_count: row.try_get("object_count").map_err(db)?,
        triple_count: row.try_get("triple_count").map_err(db)?,
    })
}

fn db(e: sqlx::Error) -> ApiError {
    ApiError::Domain(sbol_db_core::DomainError::Database(e.to_string()))
}
