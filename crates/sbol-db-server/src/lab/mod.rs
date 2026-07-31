//! Lab bench JSON API.
//!
//! Endpoints live under `/lab/api/*`. The administrator application at
//! `/admin/*` is the primary consumer; `/lab/*` remains a transitional entry
//! for old bookmarks. The JSON shape is also documented in the OpenAPI schema
//! for ad-hoc clients.
//!
//! The sub-router exposes SQL and SPARQL `execute`/`validate` pairs,
//! schema introspection for both dialects, document listing, and the
//! nested observability endpoints. See [`router`] for the full map.

mod graphs;
mod info;
mod observability;
mod overview;
mod schema;
mod sparql;
mod sql;
mod validate;

pub use schema::SchemaCache;

use axum::extract::{Extension, Request};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;

use crate::error::ApiError;
use crate::v2::auth::Identity;
use crate::AppState;

/// Lab API sub-router. The host server nests this under `/lab/api`,
/// ahead of the catchall asset handler so the JSON routes match
/// before the SPA fallback consumes them.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/info", get(info::handler))
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

/// Require an administrator identity for every data-lab endpoint. Identity is
/// attached by the same bearer/cookie middleware as V2, so the admin UI and API
/// clients share one account and token model.
pub(crate) async fn require_admin(
    Extension(identity): Extension<Identity>,
    req: Request,
    next: Next,
) -> Response {
    match identity.0 {
        Some(user) if user.is_admin => next.run(req).await,
        Some(_) => {
            ApiError::Forbidden("administrator access is required".to_owned()).into_response()
        }
        None => ApiError::Unauthorized("authentication is required".to_owned()).into_response(),
    }
}
