//! Schema migration for the RocksDB backend.
//!
//! RocksDB is schemaless: every column family is created when the database is
//! opened, so there is nothing to apply. The migrator records a version marker
//! so the CLI's `db migrate` and `db status` commands have something to report.

use async_trait::async_trait;
use sbol_db_core::DomainError;
use sbol_db_storage::{MigrationEntry, Migrator};

use crate::db::Db;

const SCHEMA_VERSION: i64 = 1;

#[derive(Clone)]
pub struct RocksdbMigrator {
    db: Db,
}

impl RocksdbMigrator {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Migrator for RocksdbMigrator {
    async fn run_migrations(&self) -> Result<(), DomainError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            db.put_cf("meta", b"schema_version", &SCHEMA_VERSION.to_be_bytes())
        })
        .await
        .map_err(|e| DomainError::Database(format!("rocksdb task panicked: {e}")))?
    }

    async fn migration_status(&self) -> Result<Vec<MigrationEntry>, DomainError> {
        let db = self.db.clone();
        let applied = tokio::task::spawn_blocking(move || db.exists_cf("meta", b"schema_version"))
            .await
            .map_err(|e| DomainError::Database(format!("rocksdb task panicked: {e}")))??;
        Ok(vec![MigrationEntry {
            version: SCHEMA_VERSION,
            description: "rocksdb column families".to_owned(),
            applied,
        }])
    }
}
