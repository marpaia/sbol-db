//! LSM-tree maintenance for the RocksDB engine: a point-in-time snapshot of the
//! store's on-disk shape and compaction pressure, plus a manual full compaction.
//!
//! Per-family size and file counts and the per-level breakdown come from the
//! engine's live SST file list, which tags every file with its column family,
//! level, and size. Estimated keys and compaction pressure come from integer
//! RocksDB properties read per family.

use std::collections::BTreeMap;

use async_trait::async_trait;
use sbol_db_core::DomainError;
use sbol_db_storage::{ColumnFamilyStats, LevelStats, LsmOverview, LsmStats};

use crate::db::{Db, COLUMN_FAMILIES};

/// Reports RocksDB engine internals and triggers compaction.
#[derive(Clone)]
pub struct RocksdbStats {
    db: Db,
}

impl RocksdbStats {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl LsmStats for RocksdbStats {
    async fn overview(&self) -> Result<LsmOverview, DomainError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || build_overview(&db))
            .await
            .map_err(|e| DomainError::Database(format!("rocksdb task panicked: {e}")))?
    }

    async fn compact(&self) -> Result<(), DomainError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || db.compact_all())
            .await
            .map_err(|e| DomainError::Database(format!("rocksdb task panicked: {e}")))
    }
}

/// Size and file count of one column family, summed from its live SST files.
#[derive(Default)]
struct CfFiles {
    size_bytes: i64,
    num_files: i64,
}

fn build_overview(db: &Db) -> Result<LsmOverview, DomainError> {
    // Per-family size and file counts, and per-level totals, both read from the
    // single live-file list so they always agree.
    let mut per_cf: BTreeMap<String, CfFiles> = BTreeMap::new();
    let mut per_level: BTreeMap<i32, LevelStats> = BTreeMap::new();
    for file in db.live_files()? {
        let cf = per_cf.entry(file.column_family_name.clone()).or_default();
        cf.size_bytes += file.size as i64;
        cf.num_files += 1;

        let level = per_level.entry(file.level).or_insert(LevelStats {
            level: file.level,
            num_files: 0,
            size_bytes: 0,
        });
        level.num_files += 1;
        level.size_bytes += file.size as i64;
    }

    let mut column_families = Vec::new();
    let mut total_bytes = 0i64;
    let mut estimated_keys = 0i64;
    let mut pending_compaction_bytes = 0i64;
    for name in COLUMN_FAMILIES {
        let files = per_cf.remove(*name).unwrap_or_default();
        let keys = db
            .property_int_cf(name, "rocksdb.estimate-num-keys")
            .unwrap_or(0) as i64;
        pending_compaction_bytes += db
            .property_int_cf(name, "rocksdb.estimate-pending-compaction-bytes")
            .unwrap_or(0) as i64;

        // An empty family carries no operational signal, so it stays out of the
        // breakdown to keep the list legible.
        if files.num_files == 0 && files.size_bytes == 0 && keys == 0 {
            continue;
        }

        total_bytes += files.size_bytes;
        estimated_keys += keys;
        column_families.push(ColumnFamilyStats {
            name: (*name).to_owned(),
            num_files: files.num_files,
            size_bytes: files.size_bytes,
            estimated_keys: keys,
        });
    }
    column_families.sort_by_key(|cf| std::cmp::Reverse(cf.size_bytes));

    // Running compactions are a database-wide count; reading it on the default
    // family reports the whole engine.
    let running_compactions = db
        .property_int_cf("meta", "rocksdb.num-running-compactions")
        .unwrap_or(0) as i64;

    let levels = per_level.into_values().collect();

    Ok(LsmOverview {
        total_bytes,
        estimated_keys,
        pending_compaction_bytes,
        running_compactions,
        column_families,
        levels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sbol_db_storage::LsmStats;

    #[tokio::test]
    async fn overview_and_compact_on_fresh_store() {
        let dir = std::env::temp_dir().join(format!("sbol-db-stats-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = format!("rocksdb://{}", dir.display());

        let db = crate::connect(&conn).expect("open temp rocksdb");
        let stats = RocksdbStats::new(db);

        let overview = stats.overview().await.expect("overview succeeds");
        assert!(overview.total_bytes >= 0);
        assert!(overview.estimated_keys >= 0);

        stats.compact().await.expect("compact succeeds");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
