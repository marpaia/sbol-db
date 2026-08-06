//! Administrator account lifecycle with safe projections and role invariants.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use sbol_db_app::{AdminAuditOutcome, Registration};
use sbol_db_core::{User, UserId};
use sbol_db_storage::UserListQuery;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::auth::Identity;
use super::super::error::V2Error;
use super::super::util::parse_json;
use super::confirmation;
use crate::error::ApiError;
use crate::AppState;

#[derive(Debug, Serialize)]
pub(super) struct UserResponse {
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

impl From<User> for UserResponse {
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
struct CreateUser {
    username: Option<String>,
    name: Option<String>,
    email: Option<String>,
    affiliation: Option<String>,
    password: Option<String>,
    is_admin: bool,
    is_curator: bool,
    #[serde(default = "default_true")]
    is_member: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PatchUser {
    name: Option<String>,
    email: Option<String>,
    affiliation: Option<String>,
    is_admin: Option<bool>,
    is_curator: Option<bool>,
    is_member: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DeleteUser {
    confirmation: String,
}

fn default_true() -> bool {
    true
}

const DEFAULT_PAGE_SIZE: usize = 25;
const MAX_PAGE_SIZE: usize = 100;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ListUsersQuery {
    q: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}

pub(super) async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListUsersQuery>,
) -> Result<Json<serde_json::Value>, V2Error> {
    let limit = query
        .limit
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    let requested_offset = query.offset.unwrap_or(0);
    let page = state
        .app
        .users
        .page_users(&UserListQuery {
            text: query.q,
            limit: limit as u32,
            offset: requested_offset as u64,
        })
        .await?;
    let total = page.total as usize;
    let offset = requested_offset.min(total);
    let items: Vec<UserResponse> = page.items.into_iter().map(Into::into).collect();
    Ok(Json(json!({
        "total": total,
        "limit": limit,
        "offset": offset,
        "items": items,
    })))
}

pub(super) async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    body: Bytes,
) -> Result<impl IntoResponse, V2Error> {
    let request: CreateUser = parse_json(&body)?;
    let username = required(request.username, "username")?;
    let name = required(request.name, "name")?;
    let email = required(request.email, "email")?;
    let password = required(request.password, "password")?;
    let actor = actor(&identity);
    let audit = state.app.admin_audit_service();
    audit
        .record(
            "user.create",
            actor,
            &username,
            AdminAuditOutcome::Attempted,
            None,
        )
        .await?;
    let user = state
        .app
        .auth
        .register(Registration {
            username: username.clone(),
            name,
            email,
            affiliation: normalized_optional(request.affiliation),
            password,
            is_admin: request.is_admin,
            is_curator: request.is_curator,
            is_member: request.is_member,
        })
        .await?;
    audit
        .record(
            "user.create",
            actor,
            &username,
            AdminAuditOutcome::Succeeded,
            None,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(UserResponse::from(user))))
}

pub(super) async fn patch(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(username): Path<String>,
    body: Bytes,
) -> Result<Json<UserResponse>, V2Error> {
    let request: PatchUser = parse_json(&body)?;
    let users = state.app.users.list_users().await?;
    let mut user = users
        .iter()
        .find(|candidate| candidate.username == username)
        .cloned()
        .ok_or_else(|| ApiError::NotFound(format!("user {username}")))?;

    if request.is_admin == Some(false) && user.is_admin {
        if identity.0.as_ref().map(|caller| caller.id) == Some(user.id) {
            return Err(ApiError::BadRequest(
                "an administrator cannot demote their own active account".to_owned(),
            )
            .into());
        }
        let admin_count = users.iter().filter(|candidate| candidate.is_admin).count();
        if admin_count <= 1 {
            return Err(ApiError::BadRequest(
                "the final administrator cannot be demoted".to_owned(),
            )
            .into());
        }
    }

    if let Some(name) = request.name {
        user.name = required(Some(name), "name")?;
    }
    if let Some(email) = request.email {
        user.email = required(Some(email), "email")?;
    }
    if let Some(affiliation) = request.affiliation {
        user.affiliation = normalized_optional(Some(affiliation));
    }
    if let Some(is_admin) = request.is_admin {
        user.is_admin = is_admin;
    }
    if let Some(is_curator) = request.is_curator {
        user.is_curator = is_curator;
    }
    if let Some(is_member) = request.is_member {
        user.is_member = is_member;
    }

    let actor = actor(&identity);
    let audit = state.app.admin_audit_service();
    audit
        .record(
            "user.update",
            actor,
            &username,
            AdminAuditOutcome::Attempted,
            None,
        )
        .await?;
    let updated = state.app.users.update_user(&user).await?;
    audit
        .record(
            "user.update",
            actor,
            &username,
            AdminAuditOutcome::Succeeded,
            None,
        )
        .await?;
    Ok(Json(updated.into()))
}

pub(super) async fn delete(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(username): Path<String>,
    body: Bytes,
) -> Result<StatusCode, V2Error> {
    let request: DeleteUser = parse_json(&body)?;
    confirmation(&request.confirmation, &format!("DELETE {username}"))?;
    let users = state.app.users.list_users().await?;
    let user = users
        .iter()
        .find(|candidate| candidate.username == username)
        .cloned()
        .ok_or_else(|| ApiError::NotFound(format!("user {username}")))?;
    if identity.0.as_ref().map(|caller| caller.id) == Some(user.id) {
        return Err(ApiError::BadRequest(
            "an administrator cannot delete their own active account".to_owned(),
        )
        .into());
    }
    if user.is_admin && users.iter().filter(|candidate| candidate.is_admin).count() <= 1 {
        return Err(
            ApiError::BadRequest("the final administrator cannot be deleted".to_owned()).into(),
        );
    }

    let actor = actor(&identity);
    let audit = state.app.admin_audit_service();
    audit
        .record(
            "user.delete",
            actor,
            &username,
            AdminAuditOutcome::Attempted,
            None,
        )
        .await?;
    if !state.app.users.delete_user(user.id).await? {
        audit
            .record(
                "user.delete",
                actor,
                &username,
                AdminAuditOutcome::Failed,
                Some("account disappeared before deletion"),
            )
            .await?;
        return Err(ApiError::NotFound(format!("user {username}")).into());
    }
    audit
        .record(
            "user.delete",
            actor,
            &username,
            AdminAuditOutcome::Succeeded,
            None,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

fn actor(identity: &Identity) -> &str {
    identity
        .0
        .as_ref()
        .map(|user| user.username.as_str())
        .unwrap_or("unknown")
}

fn required(value: Option<String>, field: &str) -> Result<String, V2Error> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::BadRequest(format!("{field} is required")).into())
}

fn normalized_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}
