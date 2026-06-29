//! What a backend can do, as the lab UI needs to know it.
//!
//! The lab UI is one binary served over whichever backend the operator
//! opened. Some surfaces are universal (SPARQL, the graph browser, the job
//! queue); others depend on the engine (a SQL console needs a SQL engine; a
//! relational schema browser needs tables; the maintenance page is shaped
//! differently for a relational engine than for an LSM key-value store).
//! [`Capabilities`] is the single descriptor the server hands the UI so the
//! UI shows exactly the features the running backend supports.

use serde::Serialize;

/// The storage engine behind a running server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    Postgres,
    Sqlite,
    Rocksdb,
}

impl BackendKind {
    /// Lowercase identifier (`"postgres"`, `"sqlite"`, `"rocksdb"`), matching
    /// the connection-string scheme and the JSON the UI reads.
    pub fn as_str(self) -> &'static str {
        match self {
            BackendKind::Postgres => "postgres",
            BackendKind::Sqlite => "sqlite",
            BackendKind::Rocksdb => "rocksdb",
        }
    }

    /// Human-facing engine name for UI copy (`"PostgreSQL"`, `"SQLite"`,
    /// `"RocksDB"`).
    pub fn display_name(self) -> &'static str {
        match self {
            BackendKind::Postgres => "PostgreSQL",
            BackendKind::Sqlite => "SQLite",
            BackendKind::Rocksdb => "RocksDB",
        }
    }
}

/// How a backend's maintenance surface is shaped. A relational engine reports
/// tables, indexes, and (for Postgres) sessions and locks; an LSM key-value
/// store reports column families, levels, and compaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MaintenanceStyle {
    Relational,
    Lsm,
}

/// The feature flags the lab UI gates on. Every field answers a yes/no the UI
/// asks before rendering a page, a nav item, or a column.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Capabilities {
    /// Arbitrary SQL can be executed (the SQL console). SQL engines only.
    pub sql_console: bool,
    /// The engine exposes a relational schema (tables/columns) to browse.
    pub relational_schema: bool,
    /// The maintenance page's shape, or `None` when the backend exposes no
    /// engine internals.
    pub maintenance: Option<MaintenanceStyle>,
    /// Per-statement timing (slow-query stats) is available. Postgres with
    /// `pg_stat_statements`; never on the others.
    pub slow_query_stats: bool,
    /// Live sessions and lock waits are observable. A single-writer or
    /// embedded engine has no equivalent, so this is Postgres-only.
    pub activity_and_locks: bool,
}
