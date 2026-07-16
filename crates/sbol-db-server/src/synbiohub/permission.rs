//! SynBioHub v1 object-sharing routes: addOwner and removeOwner.
//!
//! `POST <uri>/addOwner` grants another user view access to an object;
//! `<uri>/removeOwner/:username` revokes it. Both resolve the target user's
//! graph from the identity store and delegate to the facade
//! [`PermissionService`], which gates the caller on ownership. An anonymous
//! caller is `403`; an unknown target user is `404`.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use sbol_db_app::PermissionService;
use serde::Deserialize;

use super::routes::{public_uri, user_uri, PublicObject, UserObject};
use super::CurrentUser;
use crate::{ApiError, AppState};

/// Build the [`PermissionService`] from the shared facade handles.
fn service(state: &AppState) -> PermissionService {
    PermissionService::new(
        state.app.sparql_update.clone(),
        state.app.acl_service.clone(),
    )
}

/// The authenticated caller, or a `403` for an anonymous mutation attempt.
fn require_user(user: Option<sbol_db_core::User>) -> Result<sbol_db_core::User, ApiError> {
    user.ok_or_else(|| ApiError::Forbidden("authentication is required to share".to_owned()))
}

fn success() -> Response {
    (axum::http::StatusCode::OK, "Success").into_response()
}

/// The `{user}` body of an addOwner: the grantee, a username, an email, or a
/// user URI (classic accepts any and keys on the trailing segment).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AddOwnerForm {
    pub user: Option<String>,
}

/// A user object path plus the trailing `:username` grantee segment.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserRemoveOwnerPath {
    pub user_id: String,
    pub collection_id: String,
    pub display_id: String,
    pub version: String,
    pub username: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicRemoveOwnerPath {
    pub collection_id: String,
    pub display_id: String,
    pub version: String,
    pub username: String,
}

pub async fn user_add_owner(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<UserObject>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    add_owner(state, user, user_uri(&object), &headers, &body).await
}

pub async fn public_add_owner(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<PublicObject>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    add_owner(state, user, public_uri(&object), &headers, &body).await
}

pub async fn user_remove_owner(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(p): Path<UserRemoveOwnerPath>,
) -> Result<Response, ApiError> {
    let uri = format!(
        "http://synbiohub.org/user/{}/{}/{}/{}",
        p.user_id, p.collection_id, p.display_id, p.version
    );
    remove_owner(state, user, uri, &p.username).await
}

pub async fn public_remove_owner(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(p): Path<PublicRemoveOwnerPath>,
) -> Result<Response, ApiError> {
    let uri = format!(
        "http://synbiohub.org/public/{}/{}/{}",
        p.collection_id, p.display_id, p.version
    );
    remove_owner(state, user, uri, &p.username).await
}

async fn add_owner(
    state: AppState,
    user: Option<sbol_db_core::User>,
    object_uri: String,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, ApiError> {
    let user = require_user(user)?;
    let form = parse_body::<AddOwnerForm>(headers, body)?;
    let identifier = form
        .user
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest("user is required".to_owned()))?;
    let target_graph = resolve_target_graph(&state, &identifier).await?;
    service(&state)
        .add_owner(&user.graph_uri, user.is_admin, &object_uri, &target_graph)
        .await?;
    Ok(success())
}

async fn remove_owner(
    state: AppState,
    user: Option<sbol_db_core::User>,
    object_uri: String,
    username: &str,
) -> Result<Response, ApiError> {
    let user = require_user(user)?;
    let target_graph = resolve_target_graph(&state, username).await?;
    service(&state)
        .remove_owner(&user.graph_uri, user.is_admin, &object_uri, &target_graph)
        .await?;
    Ok(success())
}

/// Resolve a grantee reference to its user graph URI. The reference may be a
/// bare username/email or a URI whose trailing segment is the username, mirroring
/// classic's `addOwnedBy` lookup; an unknown user is `404`.
async fn resolve_target_graph(state: &AppState, identifier: &str) -> Result<String, ApiError> {
    let key = identifier.rsplit('/').next().unwrap_or(identifier);
    let user = state
        .app
        .users
        .find_by_email_or_username(key)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("user {key} not recognized")))?;
    Ok(user.graph_uri)
}

/// Parse a request body as JSON or form-encoded per `Content-Type`. An empty
/// body yields the type's default.
fn parse_body<T: for<'de> Deserialize<'de> + Default>(
    headers: &HeaderMap,
    body: &[u8],
) -> Result<T, ApiError> {
    if body.is_empty() {
        return Ok(T::default());
    }
    let is_json = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("application/json"))
        .unwrap_or(false);
    if is_json {
        serde_json::from_slice(body)
            .map_err(|e| ApiError::BadRequest(format!("invalid JSON body: {e}")))
    } else {
        serde_urlencoded::from_bytes(body)
            .map_err(|e| ApiError::BadRequest(format!("invalid form body: {e}")))
    }
}
