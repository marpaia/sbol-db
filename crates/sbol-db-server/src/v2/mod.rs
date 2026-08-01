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

mod account;
mod admin;
pub(crate) mod auth;
mod collaboration;
mod collections;
mod docs;
mod download;
mod error;
mod instance;
mod negotiate;
mod objects;
mod reviews;
mod search;
mod sequence;
mod session;
mod util;

use axum::routing::{delete, get, post, put};
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
        .route("/account", get(account::get).patch(account::patch))
        .route("/account/password", post(account::change_password))
        .route("/account/shared", get(account::shared))
        .nest("/admin", admin::router())
        .route("/reviews", get(reviews::list))
        .route("/openapi.json", get(docs::openapi_json))
        .route("/docs", get(docs::docs_html))
        .route("/collections", post(collections::create_collection))
        .route(
            "/collections/validate",
            post(collections::validate_collection),
        )
        .route("/collections/:iri/members", post(collections::add_member))
        .route("/collections/:iri", delete(collections::delete_collection))
        .route(
            "/collections/:iri/members/:member",
            delete(collections::remove_member),
        )
        .route("/objects", get(objects::list_objects))
        .route(
            "/objects/:iri",
            get(objects::get_object)
                .patch(objects::patch_object)
                .delete(objects::delete_object),
        )
        .route("/objects/:iri/details", get(objects::get_object_details))
        .route("/objects/:iri/activity", get(reviews::activity))
        .route(
            "/objects/:iri/reviews",
            get(reviews::get).post(reviews::request),
        )
        .route("/objects/:iri/reviews/decision", post(reviews::decide))
        .route(
            "/objects/:iri/shares",
            get(collaboration::list).post(collaboration::grant),
        )
        .route("/objects/:iri/shares/:user", delete(collaboration::revoke))
        .route("/objects/:iri/owner", put(collaboration::transfer))
        .route("/objects/:iri/publish", post(objects::publish_object))
        .route("/objects/:iri/similar", get(sequence::similar))
        .route(
            "/search",
            get(search::search).post(search::structured_search),
        )
        .route("/search/facets", get(search::facets))
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
