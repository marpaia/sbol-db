//! SBOLExplorer admin shims over the native search engine.
//!
//! SBOLExplorer is internal now: the ranked-search index, PageRank, and
//! clustering are all computed in-process by the `rebuild_search_index` job, so
//! these routes are native shims rather than proxies to an external service.
//! `updateIndex` enqueues that rebuild (classic `explorerUpdateIndex.js` called
//! SBOLExplorer's `update` endpoint); `config` is a durable settings section
//! carrying the now-internal `useSBOLExplorer` / endpoint flags; `log` returns
//! the rebuild's status envelope. All are admin-gated by the router.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::{Extension, Json};
use serde_json::{json, Value};

use super::{config_err, parse_config_value, CurrentUser};
use crate::error::ApiError;
use crate::AppState;

/// The config key holding the (now-internal) SBOLExplorer settings.
const EXPLORER_KEY: &str = "sbolexplorer";

/// `POST /admin/explorerUpdateIndex` — enqueue a full ranked-search-index
/// rebuild. The native counterpart to classic's SBOLExplorer index-update call;
/// it reuses the shared reindex enqueue path.
pub async fn update_index(state: State<AppState>) -> Response {
    super::reindex(state).await
}

/// `GET /admin/explorer` — the stored SBOLExplorer settings, or a
/// native default advertising the internal engine when unset.
pub async fn get_config(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let value = state.app.config_service().get(EXPLORER_KEY).await?;
    Ok(Json(value.unwrap_or_else(
        || json!({ "engine": "native", "useSBOLExplorer": false }),
    )))
}

/// `POST /admin/explorer` — persist the SBOLExplorer settings.
pub async fn set_config(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let is_admin = user.as_ref().map(|u| u.is_admin).unwrap_or(false);
    let value = parse_config_value(&headers, &body)?;
    state
        .app
        .config_service()
        .set(is_admin, EXPLORER_KEY, &value)
        .await
        .map_err(config_err)?;
    Ok(Json(json!({ "status": "ok" })))
}

/// `GET /admin/explorerLog` — the native engine's index-rebuild log
/// envelope. The rebuild runs as the `rebuild_search_index` job; its per-run
/// detail is available through the job-logs API.
pub async fn log() -> Json<Value> {
    Json(json!({ "engine": "native", "entries": [] }))
}
