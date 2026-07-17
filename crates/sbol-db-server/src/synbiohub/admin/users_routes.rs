//! Admin user CRUD over the identity [`UserStore`](sbol_db_storage::UserStore).
//!
//! `POST /admin/createUser` registers an account (argon2-hashing the password
//! through [`AuthService`](sbol_db_app::AuthService)); `POST /admin/updateUser`
//! edits an existing account's profile and membership flags; `POST
//! /admin/deleteUser` removes one. All three are admin-gated by the router.
//! Bodies arrive as classic form posts or as JSON.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use sbol_db_app::Registration;
use sbol_db_core::User;
use serde::Deserialize;
use serde_json::json;

use super::parse_body;
use crate::error::ApiError;
use crate::AppState;

#[derive(Deserialize, Default)]
struct CreateUserBody {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    affiliation: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default, alias = "isAdmin")]
    is_admin: Option<bool>,
    #[serde(default, alias = "isCurator")]
    is_curator: Option<bool>,
    #[serde(default, alias = "isMember")]
    is_member: Option<bool>,
}

/// `POST /admin/createUser` — create an account with the given profile and
/// membership flags. A duplicate `username` or `email` is a `400`, matching
/// classic's rejection. New accounts are members unless `isMember=false`.
pub async fn create_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let form: CreateUserBody = parse_body(&headers, &body)?;
    let username = required(form.username, "username")?;
    let name = required(form.name, "name")?;
    let email = required(form.email, "email")?;
    let password = required(form.password, "password")?;

    // Pre-check so a duplicate is a deterministic 400 across backends rather
    // than a backend-specific constraint error.
    if state
        .app
        .users
        .find_by_email_or_username(&username)
        .await?
        .is_some()
        || state
            .app
            .users
            .find_by_email_or_username(&email)
            .await?
            .is_some()
    {
        return Err(ApiError::BadRequest(
            "username or email already exists".to_owned(),
        ));
    }

    let registration = Registration {
        username,
        name,
        email,
        affiliation: form.affiliation.filter(|a| !a.is_empty()),
        password,
        is_admin: form.is_admin.unwrap_or(false),
        is_curator: form.is_curator.unwrap_or(false),
        is_member: form.is_member.unwrap_or(true),
    };
    let user = state.app.auth.register(registration).await?;
    Ok((StatusCode::CREATED, Json(profile_json(&user))).into_response())
}

#[derive(Deserialize, Default)]
struct UpdateUserBody {
    /// The account to edit, matching its username or email.
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    affiliation: Option<String>,
    #[serde(default, alias = "isAdmin")]
    is_admin: Option<bool>,
    #[serde(default, alias = "isCurator")]
    is_curator: Option<bool>,
    #[serde(default, alias = "isMember")]
    is_member: Option<bool>,
}

/// `POST /admin/updateUser` — edit an existing account's display name,
/// affiliation, and membership flags. The target is located by `username` (or
/// `email`); an unknown account is a `404`.
pub async fn update_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let form: UpdateUserBody = parse_body(&headers, &body)?;
    let identifier = required(
        form.username.clone().or_else(|| form.email.clone()),
        "username",
    )?;
    let mut user = locate(&state, &identifier).await?;

    if let Some(name) = form.name.filter(|n| !n.is_empty()) {
        user.name = name;
    }
    if let Some(affiliation) = form.affiliation {
        user.affiliation = Some(affiliation).filter(|a| !a.is_empty());
    }
    if let Some(is_admin) = form.is_admin {
        user.is_admin = is_admin;
    }
    if let Some(is_curator) = form.is_curator {
        user.is_curator = is_curator;
    }
    if let Some(is_member) = form.is_member {
        user.is_member = is_member;
    }

    let updated = state.app.users.update_user(&user).await?;
    Ok(Json(profile_json(&updated)).into_response())
}

#[derive(Deserialize, Default)]
struct DeleteUserBody {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

/// `POST /admin/deleteUser` — remove an account, located by `username` (or
/// `email`). An unknown account is a `404`.
pub async fn delete_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let form: DeleteUserBody = parse_body(&headers, &body)?;
    let identifier = required(form.username.or(form.email), "username")?;
    let user = locate(&state, &identifier).await?;
    if state.app.users.delete_user(user.id).await? {
        Ok(Json(json!({ "status": "deleted", "username": user.username })).into_response())
    } else {
        Err(ApiError::NotFound(format!("user {identifier}")))
    }
}

/// Resolve an account by username or email, mapping absence to a `404`.
async fn locate(state: &AppState, identifier: &str) -> Result<User, ApiError> {
    state
        .app
        .users
        .find_by_email_or_username(identifier)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("user {identifier}")))
}

/// The account as JSON, keyed with classic's camelCase field names. `password`
/// and `resetPasswordLink` are never included.
fn profile_json(user: &User) -> serde_json::Value {
    json!({
        "username": user.username,
        "name": user.name,
        "email": user.email,
        "affiliation": user.affiliation,
        "graphUri": user.graph_uri,
        "isAdmin": user.is_admin,
        "isCurator": user.is_curator,
        "isMember": user.is_member,
    })
}

/// A required string field, rejecting an absent or blank value with a `400`.
fn required(value: Option<String>, field: &str) -> Result<String, ApiError> {
    match value {
        Some(v) if !v.trim().is_empty() => Ok(v),
        _ => Err(ApiError::BadRequest(format!("{field} is required"))),
    }
}
