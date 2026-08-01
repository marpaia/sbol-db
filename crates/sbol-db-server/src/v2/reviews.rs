//! Native curator-review resources and object-scoped audit projection.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use sbol_db_app::{AuditEvent, ReviewCase, ReviewDecision};
use sbol_db_core::{DomainError, IriString};
use serde::{Deserialize, Serialize};

use super::auth::{require_user, Identity};
use super::collaboration::{authorize_management, resolve_member};
use super::error::V2Error;
use super::util::{parse_json, required};
use crate::error::ApiError;
use crate::AppState;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ReviewRequest {
    /// Username or email of the assigned curator.
    curator: Option<String>,
    note: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ReviewDecisionRequest {
    /// `approve` or `request_changes`.
    decision: Option<String>,
    note: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct ReviewListResponse {
    items: Vec<ReviewCase>,
    total: usize,
}

#[derive(Debug, Serialize)]
pub(super) struct ActivityResponse {
    items: Vec<AuditEvent>,
    total: usize,
}

/// `GET /api/v2/reviews` — the caller's submitted/assigned review queue.
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<ReviewListResponse>, V2Error> {
    let caller = require_user(&identity)?;
    let items = state
        .app
        .review_service()
        .list_for(&caller.graph_uri, caller.is_admin)
        .await?;
    let total = items.len();
    Ok(Json(ReviewListResponse { items, total }))
}

/// `GET /api/v2/objects/{iri}/reviews` — latest review cycle for one object.
pub(super) async fn get(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(iri): Path<String>,
) -> Result<Json<ReviewCase>, V2Error> {
    IriString::new(iri.clone()).map_err(DomainError::from)?;
    let caller = require_user(&identity)?;
    let case = state
        .app
        .review_service()
        .latest_case(&iri)
        .await?
        .ok_or_else(|| V2Error::from(ApiError::NotFound(format!("review for {iri}"))))?;
    if !caller.is_admin
        && case.curator_graph != caller.graph_uri
        && case.requested_by_graph != caller.graph_uri
        && authorize_management(&state, &caller, &iri).await.is_err()
    {
        return Err(ApiError::Forbidden(format!(
            "not authorized to inspect review history for {iri}"
        ))
        .into());
    }
    Ok(Json(case))
}

/// `POST /api/v2/objects/{iri}/reviews` — request review from one curator.
pub(super) async fn request(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(iri): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, Json<ReviewCase>), V2Error> {
    IriString::new(iri.clone()).map_err(DomainError::from)?;
    let caller = require_user(&identity)?;
    let request: ReviewRequest = parse_json(&body)?;
    let curator = resolve_member(&state, &required(request.curator, "curator")?).await?;
    let case = state
        .app
        .review_service()
        .request(
            &caller.graph_uri,
            caller.is_admin,
            &iri,
            &curator.graph_uri,
            curator.is_curator,
            request.note.as_deref(),
        )
        .await?;
    Ok((StatusCode::CREATED, Json(case)))
}

/// `POST /api/v2/objects/{iri}/reviews/decision` — append a curator decision.
pub(super) async fn decide(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(iri): Path<String>,
    body: Bytes,
) -> Result<Json<ReviewCase>, V2Error> {
    IriString::new(iri.clone()).map_err(DomainError::from)?;
    let caller = require_user(&identity)?;
    let request: ReviewDecisionRequest = parse_json(&body)?;
    let decision = match required(request.decision, "decision")?.as_str() {
        "approve" => ReviewDecision::Approve,
        "request_changes" => ReviewDecision::RequestChanges,
        other => {
            return Err(ApiError::BadRequest(format!(
                "decision must be approve or request_changes, got {other}"
            ))
            .into())
        }
    };
    let case = state
        .app
        .review_service()
        .decide(
            &caller.graph_uri,
            caller.is_curator,
            caller.is_admin,
            &iri,
            decision,
            request.note.as_deref(),
        )
        .await?;
    Ok(Json(case))
}

/// `GET /api/v2/objects/{iri}/activity` — owner/admin audit evidence.
pub(super) async fn activity(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(iri): Path<String>,
) -> Result<Json<ActivityResponse>, V2Error> {
    IriString::new(iri.clone()).map_err(DomainError::from)?;
    let caller = require_user(&identity)?;
    authorize_management(&state, &caller, &iri).await?;
    let items = state.app.audit_service().for_object(&iri).await?;
    let total = items.len();
    Ok(Json(ActivityResponse { items, total }))
}
