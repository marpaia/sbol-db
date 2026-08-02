//! Administrator control plane for durable edge runtime settings.

use axum::body::Bytes;
use axum::extract::State;
use axum::{Extension, Json};
use sbol_db_app::AdminAuditOutcome;

use super::super::auth::Identity;
use super::super::error::V2Error;
use super::super::util::parse_json;
use crate::{ApiError, AppState, EdgeAdminError, EdgeAdminSnapshot, EdgeSettingsPatch};

pub(super) async fn get(State(state): State<AppState>) -> Result<Json<EdgeAdminSnapshot>, V2Error> {
    let service = service(&state)?;
    Ok(Json(service.snapshot().await.map_err(edge_error)?))
}

pub(super) async fn patch(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    body: Bytes,
) -> Result<Json<EdgeAdminSnapshot>, V2Error> {
    let request: EdgeSettingsPatch = parse_json(&body)?;
    let actor = identity
        .0
        .as_ref()
        .expect("admin middleware attaches an administrator")
        .username
        .clone();
    let audit = state.app.admin_audit_service();
    audit
        .record(
            "edge.settings.update",
            &actor,
            "edge_runtime",
            AdminAuditOutcome::Attempted,
            None,
        )
        .await?;

    let snapshot = match service(&state)?.update(request).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            audit
                .record(
                    "edge.settings.update",
                    &actor,
                    "edge_runtime",
                    AdminAuditOutcome::Failed,
                    Some(&error.to_string()),
                )
                .await?;
            return Err(edge_error(error));
        }
    };
    audit
        .record(
            "edge.settings.update",
            &actor,
            "edge_runtime",
            AdminAuditOutcome::Succeeded,
            Some(if snapshot.restart_required {
                "restart required"
            } else {
                "active settings unchanged"
            }),
        )
        .await?;
    Ok(Json(snapshot))
}

fn service(state: &AppState) -> Result<&crate::EdgeAdminService, V2Error> {
    state.config.edge_admin.as_deref().ok_or_else(|| {
        ApiError::Unavailable(
            "edge runtime management requires the production RocksDB profile".to_owned(),
        )
        .into()
    })
}

fn edge_error(error: EdgeAdminError) -> V2Error {
    match error {
        EdgeAdminError::Invalid(message) => ApiError::BadRequest(message).into(),
        EdgeAdminError::Storage(error) => error.into(),
    }
}
