//! `/stream/:id` — the async long-run handoff.
//!
//! A `GET` polls a stream backed by the registry on the
//! [`AppServices`](sbol_db_app::AppServices) facade (classic `lib/api/stream.js`):
//! while the work is in flight it answers `503` with `Retry-After: 1`, and once
//! the work resolves it serves the payload. A cleared stream is `410` and an
//! unknown one is `404`. A `DELETE` clears the stream.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{CONTENT_TYPE, RETRY_AFTER};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sbol_db_app::StreamServe;
use uuid::Uuid;

use crate::AppState;

/// `GET /stream/:id`.
pub async fn serve_stream(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Ok(id) = Uuid::parse_str(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match state.app.stream.serve(id) {
        StreamServe::Pending => {
            (StatusCode::SERVICE_UNAVAILABLE, [(RETRY_AFTER, "1")]).into_response()
        }
        StreamServe::Ready(outcome) => {
            let content_type = outcome
                .content_type
                .unwrap_or_else(|| "application/octet-stream".to_owned());
            ([(CONTENT_TYPE, content_type)], Body::from(outcome.body)).into_response()
        }
        StreamServe::Cleared => StatusCode::GONE.into_response(),
        StreamServe::Gone => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `DELETE /stream/:id`.
pub async fn clear_stream(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if let Ok(id) = Uuid::parse_str(&id) {
        state.app.stream.clear(id);
    }
    (StatusCode::OK, "Cleared.").into_response()
}
