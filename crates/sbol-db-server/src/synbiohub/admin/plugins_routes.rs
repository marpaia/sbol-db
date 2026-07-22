//! Admin plugin configuration: list, save, and delete the external plugins in
//! each of the five categories.
//!
//! The plugin lists live under one durable
//! [`ConfigStore`](sbol_db_storage::ConfigStore) key, edited through the
//! app-layer [`PluginService`](sbol_db_app::PluginService), which re-checks the
//! caller is an administrator. These routes return classic SynBioHub's
//! `text/plain` status bodies (`400` on a missing/invalid field, `404` on an
//! out-of-range index). Classic mounts `GET /admin/plugins` publicly, but every
//! `/admin/*` route here is admin-gated by the router.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use sbol_db_app::PluginError;
use serde::Deserialize;
use serde_json::Value;

use super::{parse_body, CurrentUser};
use crate::AppState;

/// Whether the resolved caller is an administrator.
fn is_admin(user: &Option<sbol_db_core::User>) -> bool {
    user.as_ref().map(|u| u.is_admin).unwrap_or(false)
}

fn text(status: StatusCode, body: impl Into<String>) -> Response {
    (status, [(CONTENT_TYPE, "text/plain")], body.into()).into_response()
}

/// Map a [`PluginError`] to the classic status code and `text/plain` body.
fn plugin_error(err: PluginError) -> Response {
    let (status, message) = match err {
        PluginError::NotAuthorized => (StatusCode::FORBIDDEN, err.to_string()),
        PluginError::MissingField(m) => (StatusCode::BAD_REQUEST, m),
        PluginError::NotFound(m) => (StatusCode::NOT_FOUND, m),
        PluginError::Contact(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        PluginError::Domain(d) => (StatusCode::INTERNAL_SERVER_ERROR, d.to_string()),
    };
    text(status, message)
}

/// `GET /admin/plugins` — the plugin lists, one array per category.
pub async fn plugins(State(state): State<AppState>) -> Response {
    match state.app.plugins().plugins().await {
        Ok(map) => Json(Value::Object(map)).into_response(),
        Err(e) => plugin_error(PluginError::Domain(e)),
    }
}

/// The `POST /admin/savePlugin` body. `id` is `New` to append or a one-based
/// index to replace.
#[derive(Debug, Default, Deserialize)]
struct SavePluginForm {
    #[serde(default)]
    category: String,
    #[serde(default = "default_id")]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: String,
}

fn default_id() -> String {
    "New".to_owned()
}

/// `POST /admin/savePlugin`.
pub async fn save_plugin(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let form: SavePluginForm = match parse_body(&headers, &body) {
        Ok(f) => f,
        Err(e) => return text(StatusCode::BAD_REQUEST, e.to_string()),
    };
    match state
        .app
        .plugins()
        .save_plugin(
            is_admin(&user),
            &form.category,
            &form.id,
            &form.name,
            &form.url,
        )
        .await
    {
        Ok(name) => text(
            StatusCode::OK,
            format!(
                "Plugin ({}, {}, {}, {}) saved successfully",
                form.id, name, form.url, form.category
            ),
        ),
        Err(e) => plugin_error(e),
    }
}

/// The `POST /admin/deletePlugin` body.
#[derive(Debug, Default, Deserialize)]
struct DeletePluginForm {
    #[serde(default)]
    category: String,
    #[serde(default)]
    id: String,
}

/// `POST /admin/deletePlugin`.
pub async fn delete_plugin(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let form: DeletePluginForm = match parse_body(&headers, &body) {
        Ok(f) => f,
        Err(e) => return text(StatusCode::BAD_REQUEST, e.to_string()),
    };
    match state
        .app
        .plugins()
        .delete_plugin(is_admin(&user), &form.category, &form.id)
        .await
    {
        Ok(()) => text(
            StatusCode::OK,
            format!(
                "Plugin ({}, {}) deleted successfully",
                form.id, form.category
            ),
        ),
        Err(e) => plugin_error(e),
    }
}
