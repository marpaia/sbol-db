//! The SQL console capability: arbitrary read/write SQL against a backend
//! whose engine is itself a SQL database.
//!
//! Postgres and SQLite provide it; the RocksDB key-value store does not.
//! Each backend enforces its own statement timeout and server-side row cap,
//! converts its native rows to JSON, and reports column types in the engine's
//! own vocabulary. The server hands the request straight through and shapes
//! the result into the lab API's wire format.
//!
//! Errors a user can cause (syntax, constraint violation, statement timeout)
//! come back as [`DomainError::InvalidInput`] so the server can answer 400
//! without the scary 5xx noise; infrastructure faults stay
//! [`DomainError::Database`] and surface as 500.

use async_trait::async_trait;
use sbol_db_core::DomainError;
use serde::Serialize;

/// A bounded SQL execution request from the lab console.
#[derive(Clone, Debug)]
pub struct SqlExecuteRequest {
    pub query: String,
    /// Per-statement timeout the backend enforces (clamped by the caller).
    pub timeout_ms: u64,
    /// Server-side cap on rows materialized into the response. Rows beyond it
    /// are dropped and reported via [`SqlExecuteResult::truncated`].
    pub row_limit: u32,
}

/// One result column, named with the engine's own type vocabulary.
#[derive(Clone, Debug, Serialize)]
pub struct SqlConsoleColumn {
    pub name: String,
    /// Engine-reported column type (`TEXT`, `INT4`, `INTEGER`, …). Used for
    /// the UI's column-header type hint and numeric right-alignment.
    pub column_type: String,
}

/// The rows and metadata for one executed statement.
#[derive(Clone, Debug, Serialize)]
pub struct SqlExecuteResult {
    pub columns: Vec<SqlConsoleColumn>,
    pub rows: Vec<Vec<serde_json::Value>>,
    /// Total rows the query produced, before the row cap was applied.
    pub row_count: u64,
    pub truncated: bool,
    pub elapsed_ms: u64,
    /// Engine session id that ran the query, when the engine has one (the
    /// Postgres backend pid). `None` for engines without a session id.
    pub backend_pid: Option<i32>,
}

/// A single syntax/parse error located in the source text. Line and column
/// are 1-indexed so Monaco can place a marker.
#[derive(Clone, Debug, Serialize)]
pub struct SqlValidateError {
    pub message: String,
    pub line: u32,
    pub column: u32,
    pub end_line: Option<u32>,
    pub end_column: Option<u32>,
}

/// Run and validate ad-hoc SQL against a SQL-engine backend.
#[async_trait]
pub trait SqlConsole: Send + Sync {
    /// Execute `req` and return up to `req.row_limit` rows.
    async fn execute(&self, req: SqlExecuteRequest) -> Result<SqlExecuteResult, DomainError>;

    /// Parse-only validation. `Ok(None)` means the text parses cleanly;
    /// `Ok(Some(_))` carries the first error's location.
    async fn validate(&self, query: &str) -> Result<Option<SqlValidateError>, DomainError>;

    /// The Monaco language id the editor uses for this engine's dialect
    /// (`"pgsql"`, `"sql"`).
    fn dialect(&self) -> &'static str;
}
