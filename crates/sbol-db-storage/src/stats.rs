//! Optional database-introspection capability.
//!
//! [`DbStats`] is the maintenance/observability surface the CLI's `inspect`
//! command and the lab's Postgres page read. It is a capability a backend may
//! or may not provide: a backend that offers it returns `Some(Arc<dyn
//! DbStats>)` from the factory, and one that does not (or cannot expose these
//! engine internals) returns `None`. The shapes here are deliberately
//! engine-flavored — they mirror what a relational engine reports about
//! tables, indexes, sessions, and locks.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sbol_db_core::DomainError;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct DatabaseSize {
    pub database: String,
    pub total_bytes: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct TableStats {
    pub name: String,
    pub rows_estimate: i64,
    pub total_bytes: i64,
    pub index_bytes: i64,
    pub n_live_tup: i64,
    pub n_dead_tup: i64,
    pub last_vacuum: Option<DateTime<Utc>>,
    pub last_autovacuum: Option<DateTime<Utc>>,
    pub last_analyze: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct IndexStats {
    pub table: String,
    pub index: String,
    pub idx_scan: i64,
    pub bytes: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Activity {
    pub pid: i32,
    pub application_name: Option<String>,
    pub state: Option<String>,
    pub wait_event_type: Option<String>,
    pub wait_event: Option<String>,
    pub query: Option<String>,
    pub query_start: Option<DateTime<Utc>>,
    pub duration_secs: Option<f64>,
    pub client_addr: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BlockingLock {
    pub blocker_pid: i32,
    pub blocker_query: Option<String>,
    pub blocked_pid: i32,
    pub blocked_query: Option<String>,
    pub mode: Option<String>,
    pub locktype: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SlowQuery {
    pub queryid: String,
    pub query: Option<String>,
    pub calls: i64,
    pub total_exec_ms: f64,
    pub mean_exec_ms: f64,
    pub rows: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct TableSchema {
    pub name: String,
    pub comment: Option<String>,
    pub columns: Vec<TableColumn>,
    pub foreign_keys_out: Vec<OutgoingForeignKey>,
    pub foreign_keys_in: Vec<IncomingForeignKey>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TableColumn {
    pub name: String,
    pub pg_type: String,
    pub nullable: bool,
    pub default_expr: Option<String>,
    pub ordinal: i32,
    pub comment: Option<String>,
    pub is_primary_key: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct OutgoingForeignKey {
    pub name: String,
    pub columns: Vec<String>,
    pub target_table: String,
    pub target_columns: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct IncomingForeignKey {
    pub name: String,
    pub source_table: String,
    pub source_columns: Vec<String>,
    pub target_columns: Vec<String>,
}

/// Read-only introspection of a storage engine's tables, indexes, sessions,
/// and locks. Provided only by backends that can expose these internals.
#[async_trait]
pub trait DbStats: Send + Sync {
    async fn database_size(&self) -> Result<DatabaseSize, DomainError>;
    async fn table_stats(&self, limit: i64, offset: i64) -> Result<Vec<TableStats>, DomainError>;
    async fn index_stats(&self, limit: i64) -> Result<Vec<IndexStats>, DomainError>;
    async fn activity(&self, limit: i64) -> Result<Vec<Activity>, DomainError>;
    async fn blocking_locks(&self) -> Result<Vec<BlockingLock>, DomainError>;
    async fn table_schema(&self, name: &str) -> Result<Option<TableSchema>, DomainError>;
    /// Whether per-statement timing (and therefore [`Self::slow_queries`]) is
    /// available on this engine right now.
    async fn has_slow_query_stats(&self) -> Result<bool, DomainError>;
    async fn slow_queries(&self, limit: i64) -> Result<Vec<SlowQuery>, DomainError>;
}
