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

use axum::routing::{get, post};
use axum::Router;
use sbol_db_postgres::SbolObjectService;
use sbol_db_sparql::SparqlEngine;

#[derive(Clone)]
pub struct AppState {
    pub service: Arc<SbolObjectService>,
    pub sparql: Arc<SparqlEngine>,
    pub metrics: Arc<Metrics>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(routes::healthz))
        .route("/readyz", get(routes::readyz))
        .route("/metrics", get(metrics::metrics_handler))
        .route("/docs", get(docs::docs_html))
        .route("/openapi.json", get(docs::openapi_json))
        .route("/documents", post(routes::create_document))
        .route("/documents/:id", get(routes::get_document))
        .route("/objects", get(routes::get_object_by_iri))
        .route("/objects/:id/rdf", get(routes::export_object))
        .route("/objects/neighborhood", get(routes::neighborhood))
        .route("/objects/neighborhood.rdf", get(routes::neighborhood_rdf))
        .route("/validation-runs", post(routes::revalidate_document))
        .route("/sparql", get(routes::sparql_get).post(routes::sparql_post))
        .route("/sequences/search", get(routes::sequence_search))
        .route(
            "/ontology",
            get(routes::ontology_list).post(routes::ontology_load),
        )
        .route("/ontology/term", get(routes::ontology_term))
        .route("/ontology/descendants", get(routes::ontology_descendants))
        .route_layer(axum::middleware::from_fn(metrics::track_metrics))
        .with_state(state)
}
