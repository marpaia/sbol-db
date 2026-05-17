//! Lab bench JSON API.
//!
//! Endpoints live under `/lab/api/*`. The TypeScript SPA (served by the
//! `sbol-db-ui` crate at `/lab/*`) is the primary consumer; the JSON
//! shape is also documented in the OpenAPI schema for ad-hoc clients.
//!
//! This module ships SQL execution in the first cut. SPARQL execution
//! reuses the existing `/sparql` endpoint for now; the corresponding
//! `/lab/api/sparql/*` shim and the cross-dialect `/validate` endpoints
//! land in PR 4.

mod cancel;
mod convert;
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
        .nest("/observability", observability::router())
}
