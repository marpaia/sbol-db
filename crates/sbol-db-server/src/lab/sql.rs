//! `POST /lab/api/sql/{execute,validate}` — the lab SQL console.
//!
//! The work (statement timeout, row cap, row→JSON conversion, parse-only
//! validation, client-disconnect cancellation) lives behind the backend's
//! [`SqlConsole`](sbol_db_storage::SqlConsole) capability, so this handler is
//! a thin shim: clamp the request to the server's configured limits, hand it
//! to the console, and shape the reply. On a key-value backend the console is
//! absent and both routes answer a clear "backend unsupported" error.

use axum::extract::State;
use axum::Json;
use sbol_db_storage::{SqlExecuteRequest, SqlExecuteResult};
use serde::Deserialize;

use crate::error::ApiError;
use crate::AppState;

use super::validate::{ValidateError, ValidateResp};

pub const DEFAULT_TIMEOUT_MS: u64 = 15_000;
pub const DEFAULT_ROW_CAP: u32 = 1_000;

#[derive(Deserialize)]
pub struct ExecuteReq {
    pub query: String,
    #[serde(default)]
    pub statement_timeout_ms: Option<u64>,
    #[serde(default)]
    pub row_limit: Option<u32>,
}

pub async fn execute(
    State(state): State<AppState>,
    Json(req): Json<ExecuteReq>,
) -> Result<Json<SqlExecuteResult>, ApiError> {
    if req.query.trim().is_empty() {
        return Err(ApiError::BadRequest("empty query".into()));
    }

    let timeout_ms = req
        .statement_timeout_ms
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(1, state.config.lab_sql_timeout_ms_max);
    let row_limit = req
        .row_limit
        .unwrap_or(DEFAULT_ROW_CAP)
        .min(state.config.lab_sql_row_cap_max);

    let console = state.require_sql_console()?;
    let result = console
        .execute(SqlExecuteRequest {
            query: req.query,
            timeout_ms,
            row_limit,
        })
        .await?;

    tracing::info!(
        elapsed_ms = result.elapsed_ms,
        rows_returned = result.rows.len(),
        rows_total = result.row_count,
        truncated = result.truncated,
        "lab sql execute"
    );

    Ok(Json(result))
}

/// `POST /lab/api/sql/validate` — parse without executing. Returns the
/// canonical [`ValidateResp`] envelope so the Monaco marker provider renders
/// the same way for both dialects.
pub async fn validate(
    State(state): State<AppState>,
    Json(req): Json<ExecuteReq>,
) -> Result<Json<ValidateResp>, ApiError> {
    let console = state.require_sql_console()?;
    match console.validate(&req.query).await? {
        None => Ok(Json(ValidateResp::ok())),
        Some(err) => Ok(Json(ValidateResp::err(ValidateError {
            message: err.message,
            line: err.line,
            column: err.column,
            end_line: err.end_line,
            end_column: err.end_column,
        }))),
    }
}
