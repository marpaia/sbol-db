//! SynBioHub v1 destructive object routes: makePublic, remove, replace,
//! removeCollection, icon.
//!
//! Classic SynBioHub exposes these as `GET`-triggered mutations on an object's
//! path (a browser-form quirk). This adapter keeps that wire shape while the
//! facade [`MutationService`] holds the real verbs; the idiomatic V2 surface will
//! use `DELETE`. Every route is identity-gated: an anonymous caller is `403`, and
//! the facade rejects a non-owner (or a non-admin editing a public object) so a
//! caller can never mutate an object it does not own.

use axum::body::Bytes;
use axum::extract::{Multipart, Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use sbol_db_app::{MakePublicRequest, MutationService};
use sbol_db_storage::ImportOverwrite;
use serde::Deserialize;
use serde_json::json;

use super::routes::{public_uri, user_uri, PublicObject, UserObject};
use super::CurrentUser;
use crate::{ApiError, AppState};

/// The form fields classic's makePublic page posts.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct MakePublicForm {
    /// The public submission id; defaults to the source collection id.
    pub id: Option<String>,
    /// The public version; defaults to the source object's version.
    pub version: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    /// Comma-separated PubMed ids.
    pub citations: Option<String>,
    /// `new` for a fresh public collection, otherwise add-to-existing.
    pub tab_state: Option<String>,
    /// The classic numeric collision policy, honored when present.
    pub overwrite_merge: Option<String>,
}

/// Build the [`MutationService`] from the shared facade handles.
fn service(state: &AppState) -> MutationService {
    MutationService::new(
        state.app.store.clone(),
        state.app.sparql_update.clone(),
        state.app.acl_service.clone(),
    )
}

/// The authenticated caller, or a `403` for an anonymous mutation attempt.
fn require_user(user: Option<sbol_db_core::User>) -> Result<sbol_db_core::User, ApiError> {
    user.ok_or_else(|| ApiError::Forbidden("authentication is required to mutate".to_owned()))
}

/// The classic plain-text success body.
fn success() -> Response {
    (axum::http::StatusCode::OK, "Success").into_response()
}

// --- makePublic --------------------------------------------------------------

/// `POST /user/:userId/:collectionId/:displayId/:version/makePublic`: publish a
/// private object to the public graph under freshly minted public URIs.
pub async fn user_make_public(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<UserObject>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let user = require_user(user)?;
    let form = parse_form(&headers, &body)?;

    let overwrite = resolve_make_public_overwrite(&form);
    let citations = parse_citations(form.citations.as_deref());
    let public_id = form
        .id
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| object.collection_id.clone());
    let version = form
        .version
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| object.version.clone());

    let request = MakePublicRequest {
        source_uri: user_uri(&object),
        owner_username: object.user_id.clone(),
        public_id,
        version,
        name: form.name.filter(|s| !s.is_empty()),
        description: form.description.filter(|s| !s.is_empty()),
        creator_name: Some(user.name.clone()),
        citations,
        overwrite,
    };

    let outcome = service(&state)
        .make_public(&user.graph_uri, user.is_admin, request)
        .await?;

    let members: Vec<&str> = outcome.members.iter().map(|m| m.as_str()).collect();
    let payload = json!({
        "collectionUri": outcome.collection_uri.as_str(),
        "members": members,
        "tripleCount": outcome.triple_count,
    });
    Ok((axum::http::StatusCode::OK, Json(payload)).into_response())
}

/// Map classic's makePublic collision policy: `tabState=new` (or an absent
/// state) prevents reusing an id/version, anything else adds to an existing
/// public collection. An explicit `overwrite_merge` code overrides.
fn resolve_make_public_overwrite(form: &MakePublicForm) -> ImportOverwrite {
    if let Some(code) = form.overwrite_merge.as_deref() {
        return match code.trim() {
            "1" => ImportOverwrite::Replace,
            "2" | "3" => ImportOverwrite::Merge,
            _ => ImportOverwrite::Fail,
        };
    }
    match form.tab_state.as_deref() {
        Some("new") | None => ImportOverwrite::Fail,
        _ => ImportOverwrite::Merge,
    }
}

// --- remove / replace / removeCollection -------------------------------------

pub async fn user_remove(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<UserObject>,
) -> Result<Response, ApiError> {
    let user = require_user(user)?;
    service(&state)
        .remove(&user.graph_uri, user.is_admin, &user_uri(&object))
        .await?;
    Ok(success())
}

pub async fn public_remove(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<PublicObject>,
) -> Result<Response, ApiError> {
    let user = require_user(user)?;
    service(&state)
        .remove(&user.graph_uri, user.is_admin, &public_uri(&object))
        .await?;
    Ok(success())
}

pub async fn user_replace(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<UserObject>,
) -> Result<Response, ApiError> {
    let user = require_user(user)?;
    service(&state)
        .replace(&user.graph_uri, user.is_admin, &user_uri(&object))
        .await?;
    Ok(success())
}

pub async fn public_replace(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<PublicObject>,
) -> Result<Response, ApiError> {
    let user = require_user(user)?;
    service(&state)
        .replace(&user.graph_uri, user.is_admin, &public_uri(&object))
        .await?;
    Ok(success())
}

pub async fn user_remove_collection(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<UserObject>,
) -> Result<Response, ApiError> {
    let user = require_user(user)?;
    service(&state)
        .remove_collection(&user.graph_uri, user.is_admin, &user_uri(&object))
        .await?;
    Ok(success())
}

pub async fn public_remove_collection(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<PublicObject>,
) -> Result<Response, ApiError> {
    let user = require_user(user)?;
    service(&state)
        .remove_collection(&user.graph_uri, user.is_admin, &public_uri(&object))
        .await?;
    Ok(success())
}

// --- icon --------------------------------------------------------------------

/// `POST <uri>/icon`: upload an icon for an object. The uploaded image lands in
/// the content-addressed blob store; the route is gated on ownership. Recording
/// which stored blob an object displays as its icon is config-driven and lands
/// with the admin config store.
pub async fn user_icon(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<UserObject>,
    multipart: Multipart,
) -> Result<Response, ApiError> {
    store_icon(&state, user, user_uri(&object), multipart).await
}

pub async fn public_icon(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    Path(object): Path<PublicObject>,
    multipart: Multipart,
) -> Result<Response, ApiError> {
    store_icon(&state, user, public_uri(&object), multipart).await
}

/// Owner-gate the request, then store the first uploaded file part in the blob
/// store. Mirrors classic `updateCollectionIcon`, which writes the icon to disk;
/// here the icon is content-addressed alongside every other attachment blob.
async fn store_icon(
    state: &AppState,
    user: Option<sbol_db_core::User>,
    uri: String,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    let user = require_user(user)?;
    if !user.is_admin
        && !state
            .app
            .acl_service
            .owns_object(&user.graph_uri, &uri)
            .await?
    {
        return Err(ApiError::Forbidden(format!(
            "not authorized to mutate {uri}"
        )));
    }

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("malformed multipart body: {e}")))?
    {
        if field.file_name().is_some() {
            let bytes = field
                .bytes()
                .await
                .map_err(|e| ApiError::BadRequest(format!("invalid icon file part: {e}")))?;
            state.app.blobs.put(&bytes).await?;
            return Ok(success());
        }
        let _ = field.bytes().await;
    }
    Err(ApiError::BadRequest(
        "no icon file part in the upload".to_owned(),
    ))
}

// --- shared helpers ----------------------------------------------------------

/// Parse the makePublic body as JSON when the `Content-Type` says so, else as
/// form-encoded, matching the auth routes. An empty body yields the default
/// (all-absent) form.
fn parse_form(headers: &HeaderMap, body: &[u8]) -> Result<MakePublicForm, ApiError> {
    if body.is_empty() {
        return Ok(MakePublicForm::default());
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

/// Split a comma-separated citations field into trimmed, non-empty PubMed ids.
fn parse_citations(raw: Option<&str>) -> Vec<String> {
    raw.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_owned)
            .collect()
    })
    .unwrap_or_default()
}
