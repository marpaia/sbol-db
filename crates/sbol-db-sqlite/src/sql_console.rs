//! The SQL console capability over SQLite: arbitrary read/write SQL with a
//! per-statement timeout, a server-side row cap, untyped row→JSON conversion,
//! and parse-only validation.
//!
//! SQLite is dynamically typed, so each cell's JSON shape is decided by what
//! decodes (integer, real, text, blob) rather than by a declared column type.
//! Errors the SQLite engine reports (syntax, constraint, missing table) are
//! almost always user SQL faults, so they map to [`DomainError::InvalidInput`];
//! pool/connection faults stay [`DomainError::Database`].

use std::time::{Duration, Instant};

use async_trait::async_trait;
use sbol_db_core::DomainError;
use sbol_db_storage::{
    SqlConsole, SqlConsoleColumn, SqlExecuteRequest, SqlExecuteResult, SqlValidateError,
};
use serde_json::Value;
use sqlx::sqlite::SqliteRow;
use sqlx::{Column, Executor, Row, TypeInfo};

use crate::SqlitePool;

/// The SQL console for a SQLite backend. Cloneable; clones share the pool.
#[derive(Clone)]
pub struct SqliteSqlConsole {
    pool: SqlitePool,
}

impl SqliteSqlConsole {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

/// Decode one cell to JSON by trying SQLite's storage classes in order:
/// integer, real, text, blob (rendered as hex). A NULL cell yields
/// [`Value::Null`]; the first decode that succeeds and is `Some` wins.
fn value_to_json(row: &SqliteRow, idx: usize) -> Value {
    if let Ok(Some(n)) = row.try_get::<Option<i64>, _>(idx) {
        return Value::from(n);
    }
    if let Ok(Some(f)) = row.try_get::<Option<f64>, _>(idx) {
        return serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null);
    }
    if let Ok(Some(s)) = row.try_get::<Option<String>, _>(idx) {
        return Value::String(s);
    }
    if let Ok(Some(b)) = row.try_get::<Option<Vec<u8>>, _>(idx) {
        return Value::String(hex::encode(b));
    }
    Value::Null
}

/// Map a sqlx error from `execute`. A `Database` error is a SQLite engine
/// complaint (syntax, constraint, no such table/column), which is a user SQL
/// fault → [`DomainError::InvalidInput`]. Everything else (pool timeout, I/O,
/// lost connection) is infrastructure → [`DomainError::Database`].
fn classify_execute_error(err: sqlx::Error) -> DomainError {
    match err {
        sqlx::Error::Database(db) => DomainError::InvalidInput(db.message().to_string()),
        other => DomainError::Database(other.to_string()),
    }
}

#[async_trait]
impl SqlConsole for SqliteSqlConsole {
    async fn execute(&self, req: SqlExecuteRequest) -> Result<SqlExecuteResult, DomainError> {
        let started = Instant::now();
        let fut = sqlx::query(&req.query).fetch_all(&self.pool);
        let rows = match tokio::time::timeout(Duration::from_millis(req.timeout_ms), fut).await {
            Ok(result) => result.map_err(classify_execute_error)?,
            Err(_) => {
                return Err(DomainError::InvalidInput(
                    "statement exceeded the time limit".to_string(),
                ))
            }
        };
        let elapsed_ms = started.elapsed().as_millis() as u64;

        let total = rows.len() as u64;
        let row_limit = req.row_limit as usize;
        let truncated = total > req.row_limit as u64;

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

        let json_rows: Vec<Vec<Value>> = rows
            .iter()
            .take(row_limit)
            .map(|row| {
                (0..row.columns().len())
                    .map(|i| value_to_json(row, i))
                    .collect()
            })
            .collect();

        Ok(SqlExecuteResult {
            columns,
            rows: json_rows,
            row_count: total,
            truncated,
            elapsed_ms,
            backend_pid: None,
        })
    }

    async fn validate(&self, query: &str) -> Result<Option<SqlValidateError>, DomainError> {
        if query.trim().is_empty() {
            return Ok(None);
        }
        let mut conn = self.pool.acquire().await.map_err(crate::pool::db_err)?;
        match conn.prepare(query).await {
            Ok(_) => Ok(None),
            Err(err) => Ok(Some(SqlValidateError {
                message: err.to_string(),
                line: 1,
                column: 1,
                end_line: None,
                end_column: None,
            })),
        }
    }

    fn dialect(&self) -> &'static str {
        "sql"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{connect, run_migrations};
    use sbol_db_storage::SqlConsole;

    async fn console() -> SqliteSqlConsole {
        let pool = connect("sqlite::memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        SqliteSqlConsole::new(pool)
    }

    fn req(query: &str, row_limit: u32) -> SqlExecuteRequest {
        SqlExecuteRequest {
            query: query.to_string(),
            timeout_ms: 5_000,
            row_limit,
        }
    }

    #[tokio::test]
    async fn execute_select_literal() {
        let console = console().await;
        let result = console.execute(req("SELECT 1 AS one", 100)).await.unwrap();
        assert_eq!(result.columns.len(), 1);
        assert_eq!(result.columns[0].name, "one");
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], Value::from(1));
        assert_eq!(result.row_count, 1);
        assert!(!result.truncated);
        assert!(result.backend_pid.is_none());
    }

    #[tokio::test]
    async fn execute_applies_row_limit() {
        let console = console().await;
        let result = console
            .execute(req("SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3", 2))
            .await
            .unwrap();
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.row_count, 3);
        assert!(result.truncated);
    }

    #[tokio::test]
    async fn execute_bad_sql_is_invalid_input() {
        let console = console().await;
        let err = console.execute(req("SELCT 1", 100)).await.unwrap_err();
        assert!(matches!(err, DomainError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn validate_reports_syntax_error() {
        let console = console().await;
        let err = console.validate("SELCT 1").await.unwrap();
        assert!(err.is_some());
        let err = err.unwrap();
        assert_eq!(err.line, 1);
        assert_eq!(err.column, 1);
    }

    #[tokio::test]
    async fn validate_accepts_valid_sql() {
        let console = console().await;
        assert!(console.validate("SELECT 1").await.unwrap().is_none());
        assert!(console.validate("   ").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn dialect_is_sql() {
        let console = console().await;
        assert_eq!(console.dialect(), "sql");
    }
}
