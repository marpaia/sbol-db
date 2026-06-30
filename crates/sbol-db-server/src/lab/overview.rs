//! `GET /lab/api/overview` — one round-trip dashboard payload.
//!
//! Collects everything the landing page needs in a single request:
//! corpus counts, the most recently created graphs, top SBOL classes by
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

use crate::error::ApiError;
use crate::AppState;

#[derive(Serialize, Clone)]
pub struct Overview {
    pub counts: Counts,
    pub recent_graphs: Vec<RecentGraph>,
    pub top_classes: Vec<TopClass>,
    pub loaded_ontologies: Vec<LoadedOntology>,
}

#[derive(Serialize, Clone, Default)]
pub struct Counts {
    pub objects: i64,
    pub graphs: i64,
    pub triples: i64,
    pub sequences: i64,
    pub validation_runs: i64,
    pub ontologies: i64,
}

#[derive(Serialize, Clone)]
pub struct RecentGraph {
    pub id: uuid::Uuid,
    pub iri: String,
    pub kind: String,
    pub name: Option<String>,
    pub source_uri: Option<String>,
    pub serialization_format: Option<String>,
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

    let c = state.lab.corpus_counts().await?;
    let counts = Counts {
        objects: c.objects,
        graphs: c.graphs,
        triples: c.triples,
        sequences: c.sequences,
        validation_runs: c.validation_runs,
        ontologies: c.ontologies,
    };

    let recent_graphs = state
        .lab
        .recent_graphs(5)
        .await?
        .into_iter()
        .map(|g| RecentGraph {
            id: g.id.0,
            iri: g.iri,
            kind: g.kind,
            name: g.name,
            source_uri: g.source_uri,
            serialization_format: g.serialization_format,
            created_at: g.created_at,
            object_count: g.object_count,
        })
        .collect();

    let top_classes = state
        .lab
        .top_classes(10)
        .await?
        .into_iter()
        .map(|c| TopClass {
            iri: c.iri,
            count: c.count,
        })
        .collect();

    let loaded_ontologies = state
        .service
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
        recent_graphs,
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
