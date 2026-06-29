//! LSM-tree maintenance: the engine-internals surface for a log-structured
//! key-value backend (RocksDB).
//!
//! A relational engine reports tables, indexes, sessions, and locks (see
//! [`crate::DbStats`]). An LSM store has none of those; what it has is a set
//! of column families, each laid out across sorted levels of immutable SST
//! files that background compaction rewrites. This trait reports that shape —
//! sizes, file counts, estimated keys, and compaction pressure — and lets an
//! operator trigger a manual full compaction.

use async_trait::async_trait;
use sbol_db_core::DomainError;
use serde::Serialize;

/// A snapshot of the whole store's on-disk shape and compaction pressure.
#[derive(Clone, Debug, Serialize)]
pub struct LsmOverview {
    /// Total live bytes across every column family's SST files.
    pub total_bytes: i64,
    /// Sum of per-family estimated key counts.
    pub estimated_keys: i64,
    /// Bytes the engine estimates pending compaction must still rewrite. A
    /// large, growing value means compaction is falling behind writes.
    pub pending_compaction_bytes: i64,
    /// Compactions running right now.
    pub running_compactions: i64,
    /// Per-column-family breakdown, largest first.
    pub column_families: Vec<ColumnFamilyStats>,
    /// Per-level breakdown aggregated across families (level 0 is newest).
    pub levels: Vec<LevelStats>,
}

/// On-disk footprint of one column family.
#[derive(Clone, Debug, Serialize)]
pub struct ColumnFamilyStats {
    pub name: String,
    pub num_files: i64,
    pub size_bytes: i64,
    pub estimated_keys: i64,
}

/// SST file count and size at one LSM level, summed across families.
#[derive(Clone, Debug, Serialize)]
pub struct LevelStats {
    pub level: i32,
    pub num_files: i64,
    pub size_bytes: i64,
}

/// Read engine internals and trigger compaction on an LSM key-value backend.
/// Provided only by backends with an LSM engine.
#[async_trait]
pub trait LsmStats: Send + Sync {
    /// A point-in-time snapshot of sizes, file counts, and compaction state.
    async fn overview(&self) -> Result<LsmOverview, DomainError>;

    /// Trigger a full manual compaction across every column family. Returns
    /// once the engine has finished rewriting; can be slow on a large store.
    async fn compact(&self) -> Result<(), DomainError>;
}
