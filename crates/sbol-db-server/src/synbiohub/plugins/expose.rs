//! `GET /expose/:id` — serve a time-limited exposed artifact to a plugin.
//!
//! The id-to-path registry with its ten-minute lifetime lives on the
//! [`AppServices`](sbol_db_app::AppServices) facade (classic `lib/api/expose.js`).
//! An unknown, expired, or malformed id is `404`.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::AppState;

/// `GET /expose/:id`.
pub async fn serve_expose(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Ok(id) = Uuid::parse_str(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(path) = state.app.expose.get(id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [(CONTENT_TYPE, "application/octet-stream")],
            Body::from(bytes),
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}
