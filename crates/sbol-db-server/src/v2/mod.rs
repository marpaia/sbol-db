//! The idiomatic V2 REST adapter, mounted under `/api/v2`.
//!
//! V2 is a second presentation of the same [`AppServices`](sbol_db_app::AppServices)
//! facade the V1 SynBioHub adapter presents; it holds no business logic of its
//! own. Every handler calls the same facade verbs the V1 adapter calls and
//! differs only in wire shape: proper HTTP verbs, JSON request and response
//! bodies, real pagination with a total, `Accept`-driven content negotiation, a
//! single consistent [`V2Error`] envelope, and bearer or same-origin browser
//! session auth. It is ACL-scoped and identity-aware through the same
//! [`GraphScope`](sbol_db_sparql::GraphScope) and account the V1 adapter uses.

pub(crate) mod auth;
mod collections;
mod docs;
mod download;
mod error;
mod instance;
mod negotiate;
mod objects;
mod search;
mod sequence;
mod session;
mod util;

use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::AppState;

/// The V2 router: the identity middleware wrapping the resource routes. Mounted
/// by [`crate::router`] under `/api/v2`, inheriting the native API's metrics,
/// body-limit, and timeout layers.
///
/// Every route delegates to the same `sbol-db-app` facade verbs the V1 adapter
/// calls; V2 differs only in wire shape (proper HTTP verbs, JSON bodies, real
/// pagination, `Accept` negotiation, a consistent error envelope, and bearer
/// or same-origin browser-session auth).
pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/", get(version))
        .route("/instance", get(instance::get))
        .route(
            "/session",
            get(session::get)
                .post(session::create)
                .delete(session::delete),
        )
        .route("/openapi.json", get(docs::openapi_json))
        .route("/docs", get(docs::docs_html))
        .route("/collections", post(collections::create_collection))
        .route("/objects", get(objects::list_objects))
        .route(
            "/objects/:iri",
            get(objects::get_object)
                .patch(objects::patch_object)
                .delete(objects::delete_object),
        )
        .route("/objects/:iri/publish", post(objects::publish_object))
        .route("/objects/:iri/similar", get(sequence::similar))
        .route(
            "/search",
            get(search::search).post(search::structured_search),
        )
        .route("/search/strategies", get(search::strategies))
        .route("/sequences/search", get(sequence::search_sequences))
        .route_layer(axum::middleware::from_fn_with_state(
            state,
            auth::attach_identity,
        ))
}

/// `GET /api/v2` — the version and identity probe. Unauthenticated; serves as
/// the surface's health endpoint.
async fn version() -> Json<Value> {
    Json(json!({
        "name": "sbol-db",
        "api": "v2",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
