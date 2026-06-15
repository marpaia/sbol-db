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
mod documents;
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
        .route("/documents", get(documents::list_documents))
        .route("/documents/:id", get(documents::get_document_detail))
        .nest("/observability", observability::router())
}
