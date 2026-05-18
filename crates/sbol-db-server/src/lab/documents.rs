//! `/lab/api/documents` — paginated listing of imported SBOL documents.
//!
//! The public API exposes `POST /documents`, `GET /documents/{id}`, and the
//! `recent_documents` slice on the overview blob, but has no "list all"
//! endpoint. The Documents route in the lab UI needs one. This handler
//! is a thin, bounded `SELECT … FROM sbol_documents` with an object-count
//! join so the listing can show the same "N objects" badge the dashboard
//! does.

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
}

#[derive(Serialize)]
pub struct DocumentSummary {
    pub id: Uuid,
    pub document_iri: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub serialization_format: String,
    pub source_uri: Option<String>,
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub object_count: i64,
    pub quad_count: i64,
}

#[derive(Serialize)]
pub struct ListResponse {
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
    pub documents: Vec<DocumentSummary>,
}

pub async fn list_documents(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<ListResponse>, ApiError> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = q.offset.unwrap_or(0).max(0);
    let pool = state.service.pool();

    let total: i64 = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sbol_documents")
        .fetch_one(pool)
        .await
        .map_err(db)?;

    let rows = sqlx::query(
        r#"
        SELECT
          d.id,
          d.document_iri,
          d.name,
          d.description,
          d.serialization_format,
          d.source_uri,
          d.created_by,
          d.created_at,
          coalesce(o.n, 0) AS object_count,
          coalesce(q.n, 0) AS quad_count
        FROM sbol_documents d
        LEFT JOIN (
          SELECT document_id, count(*) AS n
          FROM sbol_objects
          WHERE document_id IS NOT NULL
          GROUP BY document_id
        ) o ON o.document_id = d.id
        LEFT JOIN (
          SELECT document_id, count(*) AS n
          FROM sbol_quads
          WHERE document_id IS NOT NULL
          GROUP BY document_id
        ) q ON q.document_id = d.id
        ORDER BY d.created_at DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(db)?;

    let documents = rows
        .into_iter()
        .map(row_to_summary)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Json(ListResponse {
        total,
        limit,
        offset,
        documents,
    }))
}

pub async fn get_document_detail(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DocumentSummary>, ApiError> {
    let pool = state.service.pool();
    let row = sqlx::query(
        r#"
        SELECT
          d.id,
          d.document_iri,
          d.name,
          d.description,
          d.serialization_format,
          d.source_uri,
          d.created_by,
          d.created_at,
          (SELECT count(*) FROM sbol_objects WHERE document_id = d.id) AS object_count,
          (SELECT count(*) FROM sbol_quads   WHERE document_id = d.id) AS quad_count
        FROM sbol_documents d
        WHERE d.id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db)?;

    match row {
        None => Err(ApiError::NotFound(format!("document {id}"))),
        Some(row) => Ok(Json(row_to_summary(row)?)),
    }
}

fn row_to_summary(row: sqlx::postgres::PgRow) -> Result<DocumentSummary, ApiError> {
    Ok(DocumentSummary {
        id: row.try_get("id").map_err(db)?,
        document_iri: row.try_get("document_iri").map_err(db)?,
        name: row.try_get("name").map_err(db)?,
        description: row.try_get("description").map_err(db)?,
        serialization_format: row.try_get("serialization_format").map_err(db)?,
        source_uri: row.try_get("source_uri").map_err(db)?,
        created_by: row.try_get("created_by").map_err(db)?,
        created_at: row.try_get("created_at").map_err(db)?,
        object_count: row.try_get("object_count").map_err(db)?,
        quad_count: row.try_get("quad_count").map_err(db)?,
    })
}

fn db(e: sqlx::Error) -> ApiError {
    ApiError::Domain(sbol_db_core::DomainError::Database(e.to_string()))
}
