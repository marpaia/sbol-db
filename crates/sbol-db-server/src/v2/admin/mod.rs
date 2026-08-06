//! Native administrator API.
//!
//! This router is the sole privileged boundary used by the new administrator
//! UI. Compatibility endpoints remain mounted unchanged, but new code does not
//! infer authority from navigation visibility or duplicate policy per route.

mod backup;
mod catalog;
mod edge;
mod instance;
mod integrations;
mod operations;
mod users;

use axum::extract::{Extension, Query, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use super::auth::Identity;
use super::error::V2Error;
use crate::error::ApiError;
use crate::AppState;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(overview))
        .route("/instance", get(instance::get).patch(instance::patch))
        .route("/dashboard", get(catalog::dashboard))
        .route("/graphs", get(catalog::list_graphs))
        .route("/graphs/:id", get(catalog::get_graph))
        .route("/graphs/:id/triples", get(catalog::graph_triples))
        .route("/resources", get(catalog::list_resources))
        .route(
            "/resources/lookup",
            get(catalog::get_resource).post(catalog::lookup_resources),
        )
        .route("/sequences", get(catalog::list_sequences))
        .route("/users", get(users::list).post(users::create))
        .route(
            "/users/:username",
            patch(users::patch).delete(users::delete),
        )
        .route("/integrations", get(integrations::get))
        .route("/federation", post(integrations::federate))
        .route("/federation/sync", post(integrations::sync))
        .route("/registries", post(integrations::save_registry))
        .route("/registries/:uri", delete(integrations::delete_registry))
        .route("/remotes", post(integrations::save_remote))
        .route("/remotes/:id", delete(integrations::delete_remote))
        .route("/plugins", post(integrations::save_plugin))
        .route(
            "/plugins/:category/:id",
            delete(integrations::delete_plugin),
        )
        .route(
            "/jobs",
            get(operations::list_jobs).post(operations::enqueue_job),
        )
        .route("/jobs/:id", get(operations::get_job))
        .route("/jobs/:id/attempts", get(operations::job_attempts))
        .route("/jobs/:id/logs", get(operations::job_logs))
        .route("/jobs/:id/cancel", post(operations::cancel_job))
        .route(
            "/ontologies",
            get(operations::list_ontologies).post(operations::load_ontology),
        )
        .route("/search", get(operations::search_status))
        .route("/search/rebuild", post(operations::rebuild_search))
        .route("/backup", get(backup::status).post(backup::trigger))
        .route("/edge", get(edge::get).patch(edge::patch))
        .route("/audit", get(audit))
        .route_layer(axum::middleware::from_fn(require_admin))
}

/// One authorization policy for the entire SynBioHub v2 admin surface. Identity is
/// attached by the parent V2 middleware before this layer runs.
async fn require_admin(
    Extension(identity): Extension<Identity>,
    req: Request,
    next: Next,
) -> Response {
    match identity.0 {
        Some(user) if user.is_admin => next.run(req).await,
        Some(_) => V2Error::from(ApiError::Forbidden(
            "administrator access is required".to_owned(),
        ))
        .into_response(),
        None => V2Error::from(ApiError::Unauthorized(
            "authentication is required".to_owned(),
        ))
        .into_response(),
    }
}

async fn overview(State(state): State<AppState>) -> Json<Value> {
    let runtime = state.app.search_runtime();
    let body = json!({
        "api": "v2-admin",
        "policy": "authenticated_administrator",
        "sections": [
            { "id": "instance", "read": true, "mutate": true },
            { "id": "users", "read": true, "mutate": true },
            { "id": "integrations", "read": true, "mutate": true },
            { "id": "search", "read": true, "mutate": true },
            { "id": "jobs", "read": true, "mutate": true },
            { "id": "ontologies", "read": true, "mutate": true },
            { "id": "backup", "read": true, "mutate": true },
            { "id": "edge", "read": true, "mutate": true },
            { "id": "audit", "read": true, "mutate": false }
        ],
        "search": {
            "default_strategy": runtime.default_strategy(),
            "strategies": runtime.descriptors(),
        },
    });
    #[cfg(feature = "lab")]
    let body = {
        let mut body = body;
        if let Some(object) = body.as_object_mut() {
            object.insert("backend".into(), json!(state.backend_kind));
            object.insert(
                "backend_name".into(),
                json!(state.backend_kind.display_name()),
            );
            object.insert("capabilities".into(), json!(state.capabilities()));
        }
        body
    };
    Json(body)
}

#[derive(Debug, Default, Deserialize)]
struct AuditQuery {
    limit: Option<u32>,
}

async fn audit(
    State(state): State<AppState>,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Value>, V2Error> {
    let items = state
        .app
        .admin_audit_service()
        .list(query.limit.unwrap_or(100))
        .await?;
    Ok(Json(json!({ "total": items.len(), "items": items })))
}

pub(super) fn confirmation(actual: &str, expected: &str) -> Result<(), V2Error> {
    if actual == expected {
        Ok(())
    } else {
        Err(ApiError::BadRequest(format!("confirmation must exactly equal {expected:?}")).into())
    }
}
