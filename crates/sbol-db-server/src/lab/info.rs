//! `GET /lab/api/info` — which backend is running and what it can do.
//!
//! The lab SPA fetches this once on load and gates its navigation, routes,
//! and per-column rendering on the result, so it only ever shows features the
//! running backend actually supports.

use axum::extract::State;
use axum::Json;
use sbol_db_storage::{BackendKind, Capabilities};
use serde::Serialize;

use crate::AppState;

#[derive(Serialize)]
pub struct LabInfo {
    /// Engine identifier (`"postgres"`, `"sqlite"`, `"rocksdb"`).
    pub backend: BackendKind,
    /// Human-facing engine name for UI copy (`"PostgreSQL"`, …).
    pub backend_name: &'static str,
    pub capabilities: Capabilities,
}

pub async fn handler(State(state): State<AppState>) -> Json<LabInfo> {
    Json(LabInfo {
        backend: state.backend_kind,
        backend_name: state.backend_kind.display_name(),
        capabilities: state.capabilities(),
    })
}
