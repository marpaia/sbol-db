//! `POST /lab/api/sql/execute` — arbitrary read-write Postgres queries.
//!
//! There's no read-only role and no statement-shape allowlist:
//! authorization is the operator's responsibility (network ACL, reverse
//! proxy, whatever fronts the lab). What we do enforce:
//!
//! - A request-scoped `statement_timeout` (bounded by config).
//! - A `lock_timeout` so a slow `LOCK TABLE` can't pin the backend.
//! - An `idle_in_transaction_session_timeout` so a misbehaving client
//!   can't hold the txn open after the query returns.
//! - A server-side `row_limit` cap so a `SELECT *` doesn't OOM the
//!   process.
//! - Client-disconnect cancellation via [`super::cancel`], so a
//!   long-running query stops eating a connection as soon as the
//!   browser tab closes.
//!
//! Validation (parse-without-execute via `pg_query`) lands in PR 4.

use std::time::Instant;

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::AppState;

use super::validate::{offset_to_line_col, ValidateError, ValidateResp};
use super::{cancel, convert};

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

#[derive(Serialize, Clone)]
pub struct Column {
    pub name: String,
    /// Postgres type name (`TEXT`, `INT4`, `JSONB`, `UUID`, …). Stable
    /// across protocol revisions; useful for the UI's column-header
    /// type hint.
    pub pg_type: String,
}

#[derive(Serialize)]
pub struct ExecuteResp {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub row_count: u64,
    pub truncated: bool,
    pub elapsed_ms: u64,
    /// Postgres backend PID that ran the query. Returned so the UI can
    /// display it (and a future "cancel from another tab" feature can
    /// target it).
    pub backend_pid: i32,
}

pub async fn execute(
    State(state): State<AppState>,
    Json(req): Json<ExecuteReq>,
) -> Result<Json<ExecuteResp>, ApiError> {
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

    let pool = state.service.pool().clone();

    // One transaction holds the captured pid, the SET LOCAL GUCs, and
    // the query itself. The GUCs revert on commit; the pid is captured
    // up front so a CancelGuard can target it if the client disconnects
    // mid-query.
    let mut tx = pool.begin().await.map_err(db)?;

    let (pid,): (i32,) = sqlx::query_as::<sqlx::Postgres, (i32,)>("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
    let guard = cancel::install(pool.clone(), pid);

    sqlx::query::<sqlx::Postgres>(&format!("SET LOCAL statement_timeout = {timeout_ms}"))
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    sqlx::query::<sqlx::Postgres>("SET LOCAL lock_timeout = '5s'")
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    sqlx::query::<sqlx::Postgres>("SET LOCAL idle_in_transaction_session_timeout = '10s'")
        .execute(&mut *tx)
        .await
        .map_err(db)?;

    let started = Instant::now();
    let rows = sqlx::query::<sqlx::Postgres>(&req.query)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| query_err(e, pid))?;
    tx.commit().await.map_err(db)?;
    guard.disarm();

    let elapsed_ms = started.elapsed().as_millis() as u64;
    let (columns, json_rows, truncated, total) = convert::rows_to_json(&rows, row_limit);

    tracing::info!(
        elapsed_ms,
        rows_returned = json_rows.len(),
        rows_total = total,
        truncated,
        pid,
        "lab sql execute"
    );

    Ok(Json(ExecuteResp {
        columns,
        rows: json_rows,
        row_count: total,
        truncated,
        elapsed_ms,
        backend_pid: pid,
    }))
}

/// `POST /lab/api/sql/validate` — parse via libpg_query without
/// executing. Returns the canonical [`ValidateResp`] envelope so the
/// Monaco marker provider can render the same way for both dialects.
///
/// The parser used here is the *real* PostgreSQL parser (libpg_query),
/// so error messages match what the server will produce at execute
/// time. There is no semantic check (relation lookup, type inference,
/// permission), only syntax — the user finds out about
/// `relation "foo" does not exist` at Run time, not validate time.
pub async fn validate(Json(req): Json<ExecuteReq>) -> Json<ValidateResp> {
    if req.query.trim().is_empty() {
        return Json(ValidateResp::ok());
    }
    match pg_query::parse(&req.query) {
        Ok(_) => Json(ValidateResp::ok()),
        Err(err) => Json(ValidateResp::err(pg_query_to_validate(&req.query, &err))),
    }
}

fn pg_query_to_validate(source: &str, err: &pg_query::Error) -> ValidateError {
    let message = err.to_string();
    // pg_query::Error::Parse carries the libpg_query cursor position as
    // a 1-indexed byte offset; other variants don't pin a location.
    let cursor = match err {
        pg_query::Error::Parse(_) => extract_cursor_pos(&message),
        _ => None,
    };
    let (line, column) = match cursor {
        Some(pos) if pos > 0 => offset_to_line_col(source, (pos - 1) as usize),
        _ => (1, 1),
    };
    ValidateError {
        message,
        line,
        column,
        end_line: None,
        end_column: None,
    }
}

/// pg_query's `Error::Parse` Display format is
/// `"<libpg_query message> at <position>"` where `<position>` is the
/// 1-indexed byte offset of the error. Pull it back out so we can
/// translate to line/column. Returns `None` for variants without a
/// position (which we surface as line 1, column 1).
fn extract_cursor_pos(msg: &str) -> Option<u32> {
    let needle = " at ";
    let idx = msg.rfind(needle)?;
    msg[idx + needle.len()..].trim().parse::<u32>().ok()
}

fn db(e: sqlx::Error) -> ApiError {
    ApiError::Domain(sbol_db_core::DomainError::Database(e.to_string()))
}

/// Map a sqlx error to ApiError, distinguishing user-fault (bad syntax,
/// constraint violation, statement timeout) from server-fault (pool
/// exhaustion, lost connection). User-fault gets 400 so the UI can
/// render the message without scary-looking 5xx noise.
fn query_err(err: sqlx::Error, pid: i32) -> ApiError {
    if let Some(db) = err.as_database_error() {
        let code = db.code().unwrap_or_default();
        // Postgres SQLSTATE classes for the things a user can cause:
        //   22*: data exception (overflow, invalid input, …)
        //   23*: integrity constraint violation
        //   42*: syntax error or access rule violation
        //   57014: query canceled (statement_timeout or pg_cancel_backend)
        let class = code.chars().take(2).collect::<String>();
        let is_user_fault = matches!(class.as_str(), "22" | "23" | "42") || code == "57014";
        if is_user_fault {
            tracing::debug!(pid, code = %code, "lab sql user-fault error");
            return ApiError::BadRequest(db.message().to_string());
        }
    }
    db(err)
}
