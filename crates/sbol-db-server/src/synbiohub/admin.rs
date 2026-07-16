//! Search-index administration.
//!
//! `POST /admin/reindex` is the SynBioHub `updateIndex` analogue: it enqueues a
//! `rebuild_search_index` job and returns the job id for polling. The rebuild
//! itself runs on the worker, which recomputes PageRank and rebuilds the ranked
//! text index.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use sbol_db_storage::{EnqueueOutcome, NewJob};
use serde_json::json;

use super::CurrentUser;
use crate::AppState;

/// The job kind the rebuild handler is registered under.
const REINDEX_KIND: &str = "rebuild_search_index";

/// Enqueue a full ranked-search-index rebuild. Admin-only: an anonymous caller
/// is rejected with `401` and a non-admin with `403`, matching classic
/// SynBioHub's admin-gated `updateIndex`.
pub async fn reindex(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
) -> Response {
    let Some(user) = user else {
        return (StatusCode::UNAUTHORIZED, "authentication required").into_response();
    };
    if !user.is_admin {
        return (StatusCode::FORBIDDEN, "admin privileges required").into_response();
    }

    let job = NewJob {
        kind: REINDEX_KIND.to_owned(),
        payload: json!({}),
        queue: None,
        priority: None,
        max_attempts: None,
        idempotency_key: None,
        available_at: None,
        parent_job_id: None,
        correlation_id: None,
    };
    match state.jobs.enqueue(job).await {
        Ok(EnqueueOutcome::Inserted(job)) | Ok(EnqueueOutcome::AlreadyExists(job)) => {
            (StatusCode::ACCEPTED, Json(json!({ "jobId": job.id }))).into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("enqueue reindex job: {err}"),
        )
            .into_response(),
    }
}
