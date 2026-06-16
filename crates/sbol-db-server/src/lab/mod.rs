//! Lab bench JSON API.
//!
//! Endpoints live under `/lab/api/*`. The TypeScript SPA (served by the
//! `sbol-db-ui` crate at `/lab/*`) is the primary consumer; the JSON
//! shape is also documented in the OpenAPI schema for ad-hoc clients.
//!
//! The sub-router exposes SQL and SPARQL `execute`/`validate` pairs,
//! schema introspection for both dialects, document listing, and the
//! nested observability endpoints. See [`router`] for the full map.

mod cancel;
mod convert;
mod graphs;
mod observability;
mod overview;
mod schema;
mod sparql;
mod sql;
mod validate;

pub use schema::SchemaCache;

use axum::routing::{get, post};
use axum::Router;

use crate::AppState;

/// Lab API sub-router. The host server nests this under `/lab/api`,
/// ahead of the catchall asset handler so the JSON routes match
/// before the SPA fallback consumes them.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/overview", get(overview::handler))
        .route("/sql/execute", post(sql::execute))
        .route("/sql/validate", post(sql::validate))
        .route("/sparql/execute", post(sparql::execute))
        .route("/sparql/validate", post(sparql::validate))
        .route("/schema/sql", get(schema::sql))
        .route("/schema/sparql", get(schema::sparql))
        .route("/graphs", get(graphs::list_graphs))
        .route("/graphs/:id", get(graphs::get_graph_detail))
        .route("/graphs/:id/triples", get(graphs::get_graph_triples))
        .nest("/observability", observability::router())
}
