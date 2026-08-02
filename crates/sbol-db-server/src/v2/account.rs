//! Native account self-service and account-scoped workspace resources.
//!
//! Profile and password commands delegate to [`AuthService`]; shared-object
//! listing delegates to the ACL-aware application projection. Responses expose
//! no password hash, reset token, API token, or administrator configuration.

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::header::{CACHE_CONTROL, PRAGMA};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use sbol_db_app::ProfileUpdate;
use sbol_db_core::{DomainError, User, UserId};
use serde::{Deserialize, Serialize};

use super::auth::{require_user, Identity};
use super::error::V2Error;
use super::util::{parse_json, required};
use crate::error::ApiError;
use crate::AppState;

#[derive(Debug, Serialize)]
pub(super) struct AccountResponse {
    id: UserId,
    username: String,
    name: String,
    email: String,
    affiliation: Option<String>,
    graph_uri: String,
    is_admin: bool,
    is_curator: bool,
    is_member: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<User> for AccountResponse {
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            username: user.username,
            name: user.name,
            email: user.email,
            affiliation: user.affiliation,
            graph_uri: user.graph_uri,
            is_admin: user.is_admin,
            is_curator: user.is_curator,
            is_member: user.is_member,
            created_at: user.created_at,
            updated_at: user.updated_at,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AccountPatch {
    name: Option<String>,
    /// An empty string clears the affiliation; omission leaves it unchanged.
    affiliation: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PasswordChange {
    current_password: Option<String>,
    new_password: Option<String>,
}

#[derive(Debug, Serialize)]
struct SharedObjectsResponse {
    items: Vec<sbol_db_app::ObjectDetails>,
    total: usize,
    offset: usize,
    limit: usize,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct SharedPaging {
    offset: Option<usize>,
    limit: Option<usize>,
}

/// `GET /api/v2/account` — the authenticated caller's safe account profile.
pub(super) async fn get(Extension(identity): Extension<Identity>) -> Result<Response, V2Error> {
    let user = require_user(&identity)?;
    Ok(no_store(Json(AccountResponse::from(user)).into_response()))
}

/// `PATCH /api/v2/account` — update only the caller-owned profile fields.
pub(super) async fn patch(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    body: Bytes,
) -> Result<Response, V2Error> {
    let user = require_user(&identity)?;
    let patch: AccountPatch = parse_json(&body)?;
    let updated = state
        .app
        .auth
        .update_profile(
            &user,
            ProfileUpdate {
                name: patch.name,
                affiliation: patch.affiliation,
            },
        )
        .await?;
    Ok(no_store(
        Json(AccountResponse::from(updated)).into_response(),
    ))
}

/// `POST /api/v2/account/password` — re-verify the current password and replace
/// it with a fresh argon2 hash. Neither password is logged or returned.
pub(super) async fn change_password(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    body: Bytes,
) -> Result<Response, V2Error> {
    let user = require_user(&identity)?;
    let request: PasswordChange = parse_json(&body)?;
    let current = required(request.current_password, "current_password")?;
    let new = required(request.new_password, "new_password")?;
    match state
        .app
        .auth
        .change_password(&user, &current, &new, &state.config.password_salt)
        .await
    {
        Ok(()) => Ok(no_store(StatusCode::NO_CONTENT.into_response())),
        Err(DomainError::Validation(_)) => Err(V2Error::from(ApiError::BadRequest(
            "current password is incorrect".to_owned(),
        ))),
        Err(error) => Err(error.into()),
    }
}

/// `GET /api/v2/account/shared` — exact objects explicitly shared with the
/// caller, sorted by IRI and projected through the ordinary ACL boundary.
pub(super) async fn shared(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(paging): Query<SharedPaging>,
) -> Result<Response, V2Error> {
    let user = require_user(&identity)?;
    let offset = paging.offset.unwrap_or(0);
    let limit = paging.limit.unwrap_or(24).clamp(1, 100);
    let (items, total) = state
        .app
        .shared_object_details_page(&user.graph_uri, offset, limit)
        .await?;
    Ok(no_store(
        Json(SharedObjectsResponse {
            items,
            total,
            offset,
            limit,
        })
        .into_response(),
    ))
}

fn no_store(mut response: Response) -> Response {
    let headers: &mut HeaderMap = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    response
}
