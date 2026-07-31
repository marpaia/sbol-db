//! Native read-only sharing and explicit ownership transfer.
//!
//! The classic `addOwner` adapter retains its co-owner semantics. V2 separates
//! read-only shares (`sbh:canView`) from ownership stamps, so a recipient can
//! inspect a private object but cannot mutate it. Ownership transfer is one
//! explicit atomic application command.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use sbol_db_core::{DomainError, IriString, User};
use serde::{Deserialize, Serialize};

use super::auth::{require_user, scope_for, Identity};
use super::error::V2Error;
use super::util::{parse_json, required};
use crate::error::ApiError;
use crate::AppState;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CollaboratorRequest {
    /// Username or email of the recipient.
    user: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct Collaborator {
    username: String,
    name: String,
    graph_uri: String,
    is_curator: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct CollaboratorsResponse {
    owners: Vec<Collaborator>,
    viewers: Vec<Collaborator>,
}

/// `GET /api/v2/objects/{iri}/shares` — owner-only collaborator inventory.
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(iri): Path<String>,
) -> Result<Json<CollaboratorsResponse>, V2Error> {
    IriString::new(iri.clone()).map_err(DomainError::from)?;
    let caller = require_user(&identity)?;
    authorize_management(&state, &caller, &iri).await?;
    let scope = scope_for(&state, &identity).await?;
    let details = state
        .app
        .object_details(&iri, scope)
        .await?
        .ok_or_else(|| V2Error::from(ApiError::NotFound(format!("object {iri}"))))?;
    let viewer_graphs = state.app.object_viewer_graphs(&iri).await?;
    Ok(Json(CollaboratorsResponse {
        owners: resolve_graphs(&state, details.owners).await?,
        viewers: resolve_graphs(&state, viewer_graphs).await?,
    }))
}

/// `POST /api/v2/objects/{iri}/shares` — grant another member read-only access.
pub(super) async fn grant(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(iri): Path<String>,
    body: Bytes,
) -> Result<StatusCode, V2Error> {
    IriString::new(iri.clone()).map_err(DomainError::from)?;
    let caller = require_user(&identity)?;
    let request: CollaboratorRequest = parse_json(&body)?;
    let target = resolve_member(&state, &required(request.user, "user")?).await?;
    if target.id == caller.id {
        return Err(
            ApiError::BadRequest("an object is already visible to its owner".to_owned()).into(),
        );
    }
    state
        .app
        .permission_service()
        .grant_view(&caller.graph_uri, caller.is_admin, &iri, &target.graph_uri)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/v2/objects/{iri}/shares/{user}` — revoke a read-only share.
pub(super) async fn revoke(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path((iri, user)): Path<(String, String)>,
) -> Result<StatusCode, V2Error> {
    IriString::new(iri.clone()).map_err(DomainError::from)?;
    let caller = require_user(&identity)?;
    let target = resolve_member(&state, &user).await?;
    if target.id == caller.id {
        return Err(
            ApiError::BadRequest("ownership cannot be revoked as a share".to_owned()).into(),
        );
    }
    state
        .app
        .permission_service()
        .revoke_view(&caller.graph_uri, caller.is_admin, &iri, &target.graph_uri)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/v2/objects/{iri}/owner` — atomically move the caller's ownership
/// stamps to another member. Administrators who are not themselves owners may
/// not silently reassign an object through this self-service command.
pub(super) async fn transfer(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(iri): Path<String>,
    body: Bytes,
) -> Result<StatusCode, V2Error> {
    IriString::new(iri.clone()).map_err(DomainError::from)?;
    let caller = require_user(&identity)?;
    let request: CollaboratorRequest = parse_json(&body)?;
    let target = resolve_member(&state, &required(request.user, "user")?).await?;
    state
        .app
        .permission_service()
        .transfer_owner(&caller.graph_uri, caller.is_admin, &iri, &target.graph_uri)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn authorize_management(
    state: &AppState,
    caller: &User,
    iri: &str,
) -> Result<(), V2Error> {
    let graph = state
        .app
        .acl_service
        .graph_of_subject(iri)
        .await?
        .ok_or_else(|| V2Error::from(ApiError::NotFound(format!("object {iri}"))))?;
    if !state
        .app
        .acl_service
        .can_write(&caller.graph_uri, caller.is_admin, iri, &graph)
        .await?
    {
        return Err(ApiError::Forbidden(format!(
            "not authorized to manage collaborators for {iri}"
        ))
        .into());
    }
    Ok(())
}

pub(super) async fn resolve_member(state: &AppState, identifier: &str) -> Result<User, V2Error> {
    let target = state
        .app
        .users
        .find_by_email_or_username(identifier)
        .await?
        .ok_or_else(|| V2Error::from(ApiError::NotFound(format!("user {identifier}"))))?;
    if !target.is_member && !target.is_admin {
        return Err(ApiError::BadRequest(format!(
            "user {} is not an active member",
            target.username
        ))
        .into());
    }
    Ok(target)
}

async fn resolve_graphs(
    state: &AppState,
    mut graphs: Vec<String>,
) -> Result<Vec<Collaborator>, V2Error> {
    graphs.sort();
    graphs.dedup();
    let mut users = Vec::with_capacity(graphs.len());
    for graph_uri in graphs {
        let identifier = graph_uri.rsplit('/').next().unwrap_or(&graph_uri);
        if let Some(user) = state
            .app
            .users
            .find_by_email_or_username(identifier)
            .await?
        {
            users.push(Collaborator {
                username: user.username,
                name: user.name,
                graph_uri: user.graph_uri,
                is_curator: user.is_curator,
            });
        } else {
            users.push(Collaborator {
                username: identifier.to_owned(),
                name: identifier.to_owned(),
                graph_uri,
                is_curator: false,
            });
        }
    }
    users.sort_by(|left, right| left.username.cmp(&right.username));
    Ok(users)
}
