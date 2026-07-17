//! Durable instance configuration over RocksDB.
//!
//! The `app_config` column family maps a section key to a serialized
//! [`ConfigEntry`] (the value plus its `updated_at`). [`set`](ConfigStore::set)
//! overwrites the key, so a later write replaces the earlier value and refreshes
//! `updated_at`.

use async_trait::async_trait;
use chrono::Utc;
use sbol_db_core::{ConfigEntry, DomainError};
use sbol_db_storage::ConfigStore;
use serde_json::Value;

use crate::db::Db;

const CF_CONFIG: &str = "app_config";

#[derive(Clone)]
pub struct RocksdbConfigStore {
    db: Db,
}

impl RocksdbConfigStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

/// Decode a stored [`ConfigEntry`] from its JSON bytes.
fn decode_entry(bytes: &[u8]) -> Result<ConfigEntry, DomainError> {
    serde_json::from_slice(bytes)
        .map_err(|e| DomainError::Database(format!("config entry is not valid JSON: {e}")))
}

#[async_trait]
impl ConfigStore for RocksdbConfigStore {
    async fn get(&self, key: &str) -> Result<Option<Value>, DomainError> {
        let db = self.db.clone();
        let key = key.to_owned();
        blocking(move || match db.get_cf(CF_CONFIG, key.as_bytes())? {
            Some(bytes) => Ok(Some(decode_entry(&bytes)?.value)),
            None => Ok(None),
        })
        .await
    }

    async fn set(&self, key: &str, value: &Value) -> Result<(), DomainError> {
        let db = self.db.clone();
        let entry = ConfigEntry {
            key: key.to_owned(),
            value: value.clone(),
            updated_at: Utc::now(),
        };
        blocking(move || {
            let bytes = serde_json::to_vec(&entry)?;
            db.put_cf(CF_CONFIG, entry.key.as_bytes(), &bytes)
        })
        .await
    }

    async fn get_all(&self) -> Result<Vec<ConfigEntry>, DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut out = Vec::new();
            db.for_each(CF_CONFIG, |_key, value| {
                out.push(decode_entry(value)?);
                Ok(true)
            })?;
            Ok(out)
        })
        .await
    }

    async fn delete(&self, key: &str) -> Result<(), DomainError> {
        let db = self.db.clone();
        let key = key.to_owned();
        blocking(move || db.delete_cf(CF_CONFIG, key.as_bytes())).await
    }
}

async fn blocking<T, F>(f: F) -> Result<T, DomainError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, DomainError> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| DomainError::Database(format!("rocksdb task panicked: {e}")))?
}
