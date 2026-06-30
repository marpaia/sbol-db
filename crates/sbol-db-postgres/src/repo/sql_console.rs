//! The Postgres SQL console: arbitrary read-write SQL for the lab bench.
//!
//! There's no read-only role and no statement-shape allowlist:
//! authorization is the operator's responsibility (network ACL, reverse
//! proxy, whatever fronts the lab). What this enforces per request:
//!
//! - A `statement_timeout` (the caller bounds it).
//! - A `lock_timeout` so a slow `LOCK TABLE` can't pin the backend.
//! - An `idle_in_transaction_session_timeout` so a misbehaving client
//!   can't hold the txn open after the query returns.
//! - A server-side row cap so a `SELECT *` doesn't OOM the process.
//! - Client-disconnect cancellation via [`CancelGuard`], so a long-running
//!   query stops eating a connection as soon as the browser tab closes.
//!
//! Validation parses with libpg_query (the real PostgreSQL parser) without
//! executing, so the error message and position match what execution would
//! report.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use sbol_db_core::DomainError;
use sbol_db_storage::{
    SqlConsole, SqlConsoleColumn, SqlExecuteRequest, SqlExecuteResult, SqlValidateError,
};
use serde_json::Value;
use sqlx::postgres::{PgRow, PgValueRef};
use sqlx::{Column, Row, TypeInfo, ValueRef};
use tokio::runtime::Handle;

use crate::PgPool;

/// The lab SQL console over a Postgres pool.
#[derive(Clone)]
pub struct PgSqlConsole {
    pool: PgPool,
}

impl PgSqlConsole {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl SqlConsole for PgSqlConsole {
    async fn execute(&self, req: SqlExecuteRequest) -> Result<SqlExecuteResult, DomainError> {
        if req.query.trim().is_empty() {
            return Err(DomainError::InvalidInput("empty query".into()));
        }

        // One transaction holds the captured pid, the SET LOCAL GUCs, and the
        // query. The GUCs revert on commit; the pid is captured up front so a
        // CancelGuard can target it if the client disconnects mid-query.
        let mut tx = self.pool.begin().await.map_err(infra)?;

        let (pid,): (i32,) = sqlx::query_as::<sqlx::Postgres, (i32,)>("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .map_err(infra)?;
        let guard = CancelGuard::install(self.pool.clone(), pid);

        sqlx::query::<sqlx::Postgres>(&format!("SET LOCAL statement_timeout = {}", req.timeout_ms))
            .execute(&mut *tx)
            .await
            .map_err(infra)?;
        sqlx::query::<sqlx::Postgres>("SET LOCAL lock_timeout = '5s'")
            .execute(&mut *tx)
            .await
            .map_err(infra)?;
        sqlx::query::<sqlx::Postgres>("SET LOCAL idle_in_transaction_session_timeout = '10s'")
            .execute(&mut *tx)
            .await
            .map_err(infra)?;

        let started = Instant::now();
        let rows = sqlx::query::<sqlx::Postgres>(&req.query)
            .fetch_all(&mut *tx)
            .await
            .map_err(query_err)?;
        tx.commit().await.map_err(infra)?;
        guard.disarm();

        let elapsed_ms = started.elapsed().as_millis() as u64;
        let (columns, rows, truncated, row_count) = rows_to_json(&rows, req.row_limit);

        Ok(SqlExecuteResult {
            columns,
            rows,
            row_count,
            truncated,
            elapsed_ms,
            backend_pid: Some(pid),
        })
    }

    async fn validate(&self, query: &str) -> Result<Option<SqlValidateError>, DomainError> {
        if query.trim().is_empty() {
            return Ok(None);
        }
        match pg_query::parse(query) {
            Ok(_) => Ok(None),
            Err(err) => Ok(Some(pg_query_to_validate(query, &err))),
        }
    }

    fn dialect(&self) -> &'static str {
        "pgsql"
    }
}

fn pg_query_to_validate(source: &str, err: &pg_query::Error) -> SqlValidateError {
    let message = err.to_string();
    // pg_query::Error::Parse carries the libpg_query cursor position as a
    // 1-indexed byte offset; other variants don't pin a location.
    let cursor = match err {
        pg_query::Error::Parse(_) => extract_cursor_pos(&message),
        _ => None,
    };
    let (line, column) = match cursor {
        Some(pos) if pos > 0 => offset_to_line_col(source, (pos - 1) as usize),
        _ => (1, 1),
    };
    SqlValidateError {
        message,
        line,
        column,
        end_line: None,
        end_column: None,
    }
}

/// pg_query's `Error::Parse` Display format is `"<message> at <position>"`
/// where `<position>` is the 1-indexed byte offset of the error. Pull it back
/// out so we can translate to line/column. `None` for variants without one.
fn extract_cursor_pos(msg: &str) -> Option<u32> {
    let needle = " at ";
    let idx = msg.rfind(needle)?;
    msg[idx + needle.len()..].trim().parse::<u32>().ok()
}

/// Translate a byte offset into a 1-indexed (line, column) pair against the
/// original query string. Counts characters past the last newline, not bytes,
/// so multi-byte codepoints don't push columns out of register.
fn offset_to_line_col(source: &str, byte_offset: usize) -> (u32, u32) {
    let mut line: u32 = 1;
    let mut col: u32 = 1;
    let mut consumed_bytes = 0usize;
    for ch in source.chars() {
        if consumed_bytes >= byte_offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
        consumed_bytes += ch.len_utf8();
    }
    (line, col)
}

/// Map a sqlx error from the query itself, distinguishing user-fault (bad
/// syntax, constraint violation, statement timeout) from server-fault (pool
/// exhaustion, lost connection). User-fault becomes `InvalidInput` so the
/// server answers 400 without 5xx noise.
fn query_err(err: sqlx::Error) -> DomainError {
    if let Some(db) = err.as_database_error() {
        let code = db.code().unwrap_or_default();
        // Postgres SQLSTATE classes a user can cause:
        //   22*: data exception   23*: integrity constraint
        //   42*: syntax / access rule   57014: query canceled (timeout)
        let class = code.chars().take(2).collect::<String>();
        let is_user_fault = matches!(class.as_str(), "22" | "23" | "42") || code == "57014";
        if is_user_fault {
            return DomainError::InvalidInput(db.message().to_string());
        }
    }
    infra(err)
}

fn infra(e: sqlx::Error) -> DomainError {
    DomainError::Database(e.to_string())
}

/// Reshape `PgRow`s into the console wire shape. `limit` caps `rows`
/// server-side; `truncated` reports whether any were dropped; `total` is the
/// count we'd have returned without truncation.
fn rows_to_json(rows: &[PgRow], limit: u32) -> (Vec<SqlConsoleColumn>, Vec<Vec<Value>>, bool, u64) {
    let total = rows.len() as u64;
    let truncated = total > limit as u64;
    let take = rows.iter().take(limit as usize);

    let columns: Vec<SqlConsoleColumn> = rows
        .first()
        .map(|row| {
            row.columns()
                .iter()
                .map(|c| SqlConsoleColumn {
                    name: c.name().to_string(),
                    column_type: c.type_info().name().to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    let json_rows = take
        .map(|row| {
            row.columns()
                .iter()
                .enumerate()
                .map(|(i, col)| value_to_json(row, i, col.type_info().name()))
                .collect()
        })
        .collect();

    (columns, json_rows, truncated, total)
}

fn value_to_json(row: &PgRow, idx: usize, type_name: &str) -> Value {
    let raw: PgValueRef<'_> = match row.try_get_raw(idx) {
        Ok(v) => v,
        Err(_) => return Value::Null,
    };
    if raw.is_null() {
        return Value::Null;
    }

    // Dispatch on the Postgres type name (stable across protocol revisions
    // and matching what we ship in the column metadata).
    match type_name {
        "BOOL" => decode::<bool>(row, idx)
            .map(Value::Bool)
            .unwrap_or(Value::Null),
        "INT2" => decode::<i16>(row, idx)
            .map(|n| Value::from(n as i64))
            .unwrap_or(Value::Null),
        "INT4" => decode::<i32>(row, idx)
            .map(|n| Value::from(n as i64))
            .unwrap_or(Value::Null),
        "INT8" => decode::<i64>(row, idx)
            .map(Value::from)
            .unwrap_or(Value::Null),
        "FLOAT4" => decode::<f32>(row, idx)
            .and_then(|f| serde_json::Number::from_f64(f as f64).map(Value::Number))
            .unwrap_or(Value::Null),
        "FLOAT8" => decode::<f64>(row, idx)
            .and_then(|f| serde_json::Number::from_f64(f).map(Value::Number))
            .unwrap_or(Value::Null),
        // sqlx feature-gates bigdecimal/rust_decimal; we depend on neither,
        // so render NUMERIC as text.
        "NUMERIC" => decode_via_text(row, idx),
        "TEXT" | "VARCHAR" | "BPCHAR" | "NAME" | "CITEXT" => decode::<String>(row, idx)
            .map(Value::String)
            .unwrap_or(Value::Null),
        "UUID" => decode::<uuid::Uuid>(row, idx)
            .map(|u| Value::String(u.to_string()))
            .unwrap_or(Value::Null),
        "TIMESTAMPTZ" => decode::<chrono::DateTime<chrono::Utc>>(row, idx)
            .map(|t| Value::String(t.to_rfc3339()))
            .unwrap_or(Value::Null),
        "TIMESTAMP" => decode::<chrono::NaiveDateTime>(row, idx)
            .map(|t| Value::String(t.to_string()))
            .unwrap_or(Value::Null),
        "DATE" => decode::<chrono::NaiveDate>(row, idx)
            .map(|t| Value::String(t.to_string()))
            .unwrap_or(Value::Null),
        "JSON" | "JSONB" => decode::<Value>(row, idx).unwrap_or(Value::Null),
        "BYTEA" => decode::<Vec<u8>>(row, idx)
            .map(|b| Value::String(hex::encode(b)))
            .unwrap_or(Value::Null),
        "TEXT[]" | "VARCHAR[]" | "_TEXT" | "_VARCHAR" | "_BPCHAR" | "_NAME" => {
            decode::<Vec<String>>(row, idx)
                .map(|v| Value::Array(v.into_iter().map(Value::String).collect()))
                .unwrap_or(Value::Null)
        }
        "_INT4" => decode::<Vec<i32>>(row, idx)
            .map(|v| Value::Array(v.into_iter().map(|i| Value::from(i as i64)).collect()))
            .unwrap_or(Value::Null),
        "_INT8" => decode::<Vec<i64>>(row, idx)
            .map(|v| Value::Array(v.into_iter().map(Value::from).collect()))
            .unwrap_or(Value::Null),
        _ => {
            // Custom Postgres domain (sbol_iri, sbol_ontology_term, …) lands
            // here; most are text under the covers.
            if let Some(s) = decode::<String>(row, idx) {
                return Value::String(s);
            }
            decode_via_text(row, idx)
        }
    }
}

fn decode<'r, T>(row: &'r PgRow, idx: usize) -> Option<T>
where
    T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(idx).ok()
}

fn decode_via_text(row: &PgRow, idx: usize) -> Value {
    if let Ok(Some(s)) = row.try_get::<Option<String>, _>(idx) {
        return Value::String(s);
    }
    let type_name = row
        .columns()
        .get(idx)
        .map(|c| c.type_info().name().to_string())
        .unwrap_or_else(|| "unknown".into());
    Value::String(format!("<undecodable: {type_name}>"))
}

/// RAII guard that cancels backend PID `pid` on `pool` unless `disarm`ed
/// before drop. On client disconnect axum drops the request future, which
/// drops this guard and fires `pg_cancel_backend` from a fresh connection so
/// the server doesn't sit on a slot for the full timeout. The cancel runs on
/// a tokio task so `Drop` stays synchronous.
struct CancelGuard {
    inner: Arc<CancelInner>,
}

struct CancelInner {
    pool: PgPool,
    pid: i32,
    disarmed: AtomicBool,
    handle: Handle,
}

impl CancelGuard {
    fn install(pool: PgPool, pid: i32) -> Self {
        Self {
            inner: Arc::new(CancelInner {
                pool,
                pid,
                disarmed: AtomicBool::new(false),
                handle: Handle::current(),
            }),
        }
    }

    fn disarm(self) {
        self.inner.disarmed.store(true, Ordering::Release);
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if self.inner.disarmed.load(Ordering::Acquire) {
            return;
        }
        let inner = Arc::clone(&self.inner);
        inner.handle.clone().spawn(async move {
            let result = sqlx::query("SELECT pg_cancel_backend($1)")
                .bind(inner.pid)
                .execute(&inner.pool)
                .await;
            match result {
                Ok(_) => tracing::debug!(pid = inner.pid, "lab sql query cancel sent"),
                Err(e) => {
                    tracing::warn!(pid = inner.pid, error = %e, "lab sql query cancel failed")
                }
            }
        });
    }
}
