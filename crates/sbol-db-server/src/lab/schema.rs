//! `GET /lab/api/schema/{sql,sparql}` — schema introspection for the
//! lab sidebar.
//!
//! Two endpoints, both cached behind a short TTL so a chatty sidebar
//! that polls every few seconds doesn't hammer `information_schema`:
//!
//! - **SQL schema**: tables + columns in the `public` schema, pulled
//!   from `information_schema`. The sidebar uses this for click-to-
//!   insert.
//! - **SPARQL schema**: curated prefix list, augmented with any
//!   ontology prefixes loaded via `/ontology`, and the top-N
//!   `sbol_class` IRIs by row count so the user can see what's
//!   actually in the dataset.
//!
//! The TTL is intentionally short. Tables don't appear or disappear
//! often, but ontology loads do, and refreshing every minute keeps
//! the UI honest without forcing manual invalidation.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::Json;
use sbol_db_storage::RelationalSchema;
use serde::Serialize;
use tokio::sync::RwLock;

use crate::error::ApiError;
use crate::AppState;

const SCHEMA_TTL: Duration = Duration::from_secs(60);
const TOP_CLASSES_LIMIT: i64 = 25;

#[derive(Serialize, Clone)]
pub struct SparqlSchema {
    pub prefixes: Vec<SparqlPrefix>,
    pub top_classes: Vec<SparqlClass>,
}

#[derive(Serialize, Clone)]
pub struct SparqlPrefix {
    pub prefix: String,
    pub iri: String,
    /// Whether the prefix came from a loaded ontology (true) or the
    /// curated default list (false).
    pub from_ontology: bool,
}

#[derive(Serialize, Clone)]
pub struct SparqlClass {
    pub iri: String,
    pub count: i64,
}

/// Tiny per-process TTL cache. The cost we're guarding against is a
/// sidebar that polls on every focus or tab switch; one row per
/// table in `public` over a 60s window is more than fine. Replace
/// with a real listener-driven invalidation when ontology loads
/// happen often enough to matter.
#[derive(Default)]
pub struct SchemaCache {
    sql: RwLock<Option<CacheEntry<RelationalSchema>>>,
    sparql: RwLock<Option<CacheEntry<SparqlSchema>>>,
    overview: RwLock<Option<CacheEntry<super::overview::Overview>>>,
}

struct CacheEntry<T> {
    at: Instant,
    value: Arc<T>,
}

impl SchemaCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Cache accessor for `/lab/api/overview`. Defined here (rather
    /// than alongside the overview handler) so the cache primitives
    /// stay in one place — the overview shares the same TTL discipline
    /// as the schema endpoints.
    pub(super) async fn read_overview(&self) -> Option<Arc<super::overview::Overview>> {
        read_fresh(&self.overview).await
    }

    pub(super) async fn write_overview(&self, value: Arc<super::overview::Overview>) {
        *self.overview.write().await = Some(CacheEntry {
            at: Instant::now(),
            value,
        });
    }

    /// Drop all cached entries. Call this on a write that the cached
    /// payloads depend on — currently ontology loads, which change
    /// the SPARQL prefix list, the loaded-ontologies panel, and the
    /// overview's ontology count in one step.
    pub async fn invalidate_all(&self) {
        *self.sql.write().await = None;
        *self.sparql.write().await = None;
        *self.overview.write().await = None;
    }
}

pub async fn sql(State(state): State<AppState>) -> Result<Json<Arc<RelationalSchema>>, ApiError> {
    if let Some(hit) = read_fresh(&state.schema_cache.sql).await {
        return Ok(Json(hit));
    }
    let schema = state.require_db_stats()?.schema_overview().await?;
    let arc = Arc::new(schema);
    *state.schema_cache.sql.write().await = Some(CacheEntry {
        at: Instant::now(),
        value: arc.clone(),
    });
    Ok(Json(arc))
}

pub async fn sparql(State(state): State<AppState>) -> Result<Json<Arc<SparqlSchema>>, ApiError> {
    if let Some(hit) = read_fresh(&state.schema_cache.sparql).await {
        return Ok(Json(hit));
    }
    let schema = load_sparql_schema(&state).await?;
    let arc = Arc::new(schema);
    *state.schema_cache.sparql.write().await = Some(CacheEntry {
        at: Instant::now(),
        value: arc.clone(),
    });
    Ok(Json(arc))
}

async fn read_fresh<T: Clone>(cell: &RwLock<Option<CacheEntry<T>>>) -> Option<Arc<T>> {
    let guard = cell.read().await;
    let entry = guard.as_ref()?;
    if entry.at.elapsed() <= SCHEMA_TTL {
        Some(Arc::clone(&entry.value))
    } else {
        None
    }
}

async fn load_sparql_schema(state: &AppState) -> Result<SparqlSchema, ApiError> {
    let mut prefixes: Vec<SparqlPrefix> = curated_prefixes()
        .iter()
        .map(|(p, i)| SparqlPrefix {
            prefix: (*p).to_string(),
            iri: (*i).to_string(),
            from_ontology: false,
        })
        .collect();

    let loaded = state.service.list_ontologies().await?;
    for ont in loaded {
        // Skip if a curated entry already claimed the prefix.
        if prefixes
            .iter()
            .any(|p| p.prefix == ont.prefix.to_lowercase())
        {
            continue;
        }
        if let Some(iri) = ont.source_url.clone() {
            prefixes.push(SparqlPrefix {
                prefix: ont.prefix.to_lowercase(),
                iri,
                from_ontology: true,
            });
        }
    }

    let top_classes = state
        .lab
        .top_classes(TOP_CLASSES_LIMIT)
        .await?
        .into_iter()
        .map(|c| SparqlClass {
            iri: c.iri,
            count: c.count,
        })
        .collect();

    Ok(SparqlSchema {
        prefixes,
        top_classes,
    })
}

fn curated_prefixes() -> &'static [(&'static str, &'static str)] {
    &[
        ("sbol", "http://sbols.org/v3#"),
        ("prov", "http://www.w3.org/ns/prov#"),
        (
            "om",
            "http://www.ontology-of-units-of-measure.org/resource/om-2/",
        ),
        ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
        ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
        ("owl", "http://www.w3.org/2002/07/owl#"),
        ("xsd", "http://www.w3.org/2001/XMLSchema#"),
    ]
}
