//! HTTP server for sbol-db. Mirrors the CLI surface but over REST.

mod docs;
mod error;
mod export;
pub mod metrics;
mod routes;

pub use error::ApiError;
pub use export::export_subject_rdf;
pub use metrics::Metrics;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;
use sbol_db_postgres::SbolObjectService;
use sbol_db_sparql::SparqlEngine;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;

#[derive(Clone)]
pub struct AppState {
    pub service: Arc<SbolObjectService>,
    pub sparql: Arc<SparqlEngine>,
    pub metrics: Arc<Metrics>,
}

/// Operational limits applied to every route. The outer
/// `request_timeout` is a wall-clock bound on the whole request; SPARQL
/// has its own (shorter) cooperative timeout inside `SparqlOptions`.
/// `max_body_bytes` rejects oversize POST bodies before they're read
/// into memory.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub request_timeout: Duration,
    pub max_body_bytes: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            // Slightly higher than the SPARQL default (30s) so SPARQL
            // returns its 504-equivalent before the outer timer fires.
            request_timeout: Duration::from_secs(60),
            max_body_bytes: 32 * 1024 * 1024,
        }
    }
}

impl ServerConfig {
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            request_timeout: std::env::var("SBOL_DB_REQUEST_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .map(Duration::from_secs)
                .unwrap_or(defaults.request_timeout),
            max_body_bytes: std::env::var("SBOL_DB_MAX_BODY_BYTES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(defaults.max_body_bytes),
        }
    }
}

pub fn router(state: AppState, config: ServerConfig) -> Router {
    Router::new()
        .route("/healthz", get(routes::healthz))
        .route("/readyz", get(routes::readyz))
        .route("/metrics", get(metrics::metrics_handler))
        .route("/docs", get(docs::docs_html))
        .route("/openapi.json", get(docs::openapi_json))
        .route("/documents", post(routes::create_document))
        .route("/documents/bulk", post(routes::create_documents_bulk))
        .route("/documents/:id", get(routes::get_document))
        .route("/objects", get(routes::get_object_by_iri))
        .route("/objects/list", get(routes::list_objects))
        .route("/objects/lookup", post(routes::lookup_objects))
        .route("/objects/:id/rdf", get(routes::export_object))
        .route("/objects/neighborhood", get(routes::neighborhood))
        .route("/objects/neighborhood.rdf", get(routes::neighborhood_rdf))
        .route("/validation-runs", post(routes::revalidate_document))
        .route("/sparql", get(routes::sparql_get).post(routes::sparql_post))
        .route(
            "/sequences/search",
            get(routes::sequence_search).post(routes::sequence_search_batch),
        )
        .route(
            "/ontology",
            get(routes::ontology_list).post(routes::ontology_load),
        )
        .route("/ontology/term", get(routes::ontology_term))
        .route("/ontology/descendants", get(routes::ontology_descendants))
        .route_layer(axum::middleware::from_fn(metrics::track_metrics))
        // Body limit and timeout apply to every route, including the
        // operational endpoints. `DefaultBodyLimit::max` overrides axum's
        // built-in 2 MiB default; `RequestBodyLimitLayer` is the hard
        // cap that rejects oversize bodies with 413 before they're
        // streamed into memory.
        .layer(DefaultBodyLimit::max(config.max_body_bytes))
        .layer(RequestBodyLimitLayer::new(config.max_body_bytes))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            config.request_timeout,
        ))
        .with_state(state)
}
