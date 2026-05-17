//! `GET /lab/api/overview` — one round-trip dashboard payload.
//!
//! Collects everything the landing page needs in a single request:
//! corpus counts, the last few document imports, top SBOL classes by
//! row count, and the currently loaded ontologies. The cost of doing
//! this server-side (rather than five parallel client fetches) is one
//! `Vec::with_capacity` and a short txn — the cost of *not* doing it
//! is five HTTP round-trips on the dashboard render.
//!
//! Cached behind a short TTL via the same [`super::schema::SchemaCache`]
//! mechanism, so a chatty tab-switch-heavy session doesn't hammer the
//! database.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Row;

use crate::error::ApiError;
use crate::AppState;

#[derive(Serialize, Clone)]
pub struct Overview {
    pub counts: Counts,
    pub recent_documents: Vec<RecentDocument>,
    pub top_classes: Vec<TopClass>,
    pub loaded_ontologies: Vec<LoadedOntology>,
}

#[derive(Serialize, Clone, Default)]
pub struct Counts {
    pub objects: i64,
    pub documents: i64,
    pub quads: i64,
    pub sequences: i64,
    pub validation_runs: i64,
    pub ontologies: i64,
}

#[derive(Serialize, Clone)]
pub struct RecentDocument {
    pub id: uuid::Uuid,
    pub name: Option<String>,
    pub source_uri: Option<String>,
    pub serialization_format: String,
    pub created_at: DateTime<Utc>,
    pub object_count: i64,
}

#[derive(Serialize, Clone)]
pub struct TopClass {
    pub iri: String,
    pub count: i64,
}

#[derive(Serialize, Clone)]
pub struct LoadedOntology {
    pub prefix: String,
    pub name: String,
    pub term_count: i32,
}

pub async fn handler(State(state): State<AppState>) -> Result<Json<Arc<Overview>>, ApiError> {
    if let Some(hit) = state.schema_cache.read_overview().await {
        return Ok(Json(hit));
    }
    let started = Instant::now();
    let pool = state.service.pool();

    // Run the count queries in a single statement so we pay one round
    // trip + one parse plan, not six. SELECT-from-no-table is cheap
    // and the per-table `count(*)` is index-only on the primary key.
    let counts_row = sqlx::query::<sqlx::Postgres>(
        r#"
        SELECT
          (SELECT count(*) FROM sbol_objects)         AS objects,
          (SELECT count(*) FROM sbol_documents)       AS documents,
          (SELECT count(*) FROM sbol_quads)           AS quads,
          (SELECT count(*) FROM sbol_sequences)       AS sequences,
          (SELECT count(*) FROM sbol_validation_runs) AS validation_runs,
          (SELECT count(*) FROM sbol_ontologies)      AS ontologies
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(db)?;

    let counts = Counts {
        objects: counts_row.try_get("objects").map_err(db)?,
        documents: counts_row.try_get("documents").map_err(db)?,
        quads: counts_row.try_get("quads").map_err(db)?,
        sequences: counts_row.try_get("sequences").map_err(db)?,
        validation_runs: counts_row.try_get("validation_runs").map_err(db)?,
        ontologies: counts_row.try_get("ontologies").map_err(db)?,
    };

    let recent_rows = sqlx::query::<sqlx::Postgres>(
        r#"
        SELECT
          d.id,
          d.name,
          d.source_uri,
          d.serialization_format,
          d.created_at,
          coalesce(o.n, 0) AS object_count
        FROM sbol_documents d
        LEFT JOIN (
          SELECT document_id, count(*) AS n
          FROM sbol_objects
          WHERE document_id IS NOT NULL
          GROUP BY document_id
        ) o ON o.document_id = d.id
        ORDER BY d.created_at DESC
        LIMIT 5
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(db)?;

    let recent_documents = recent_rows
        .into_iter()
        .map(|row| {
            Ok::<_, ApiError>(RecentDocument {
                id: row.try_get("id").map_err(db)?,
                name: row.try_get("name").map_err(db)?,
                source_uri: row.try_get("source_uri").map_err(db)?,
                serialization_format: row.try_get("serialization_format").map_err(db)?,
                created_at: row.try_get("created_at").map_err(db)?,
                object_count: row.try_get("object_count").map_err(db)?,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let top_rows = sqlx::query::<sqlx::Postgres>(
        r#"
        SELECT sbol_class, count(*) AS n
        FROM sbol_objects
        WHERE sbol_class IS NOT NULL
        GROUP BY sbol_class
        ORDER BY n DESC
        LIMIT 10
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(db)?;

    let top_classes = top_rows
        .into_iter()
        .map(|row| {
            Ok::<_, ApiError>(TopClass {
                iri: row.try_get("sbol_class").map_err(db)?,
                count: row.try_get("n").map_err(db)?,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let loaded_ontologies = state
        .service
        .ontology()
        .list_ontologies()
        .await?
        .into_iter()
        .map(|o| LoadedOntology {
            prefix: o.prefix,
            name: o.name,
            term_count: o.term_count,
        })
        .collect();

    let overview = Overview {
        counts,
        recent_documents,
        top_classes,
        loaded_ontologies,
    };
    let arc = Arc::new(overview);
    state.schema_cache.write_overview(arc.clone()).await;
    tracing::debug!(
        elapsed_ms = started.elapsed().as_millis() as u64,
        "lab overview computed"
    );
    Ok(Json(arc))
}

fn db(e: sqlx::Error) -> ApiError {
    ApiError::Domain(sbol_db_core::DomainError::Database(e.to_string()))
}
