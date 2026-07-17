//! Durable instance configuration over SQLite.
//!
//! Entries live in `sbh_app_config`, mapping a key to a TEXT column holding the
//! serialized JSON value. [`set`](ConfigStore::set) upserts on the primary key,
//! so a later write to a key overwrites its value and refreshes `updated_at`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sbol_db_core::{ConfigEntry, DomainError};
use sbol_db_storage::ConfigStore;
use serde_json::Value;
use sqlx::Row;

use crate::pool::db_err;
use crate::SqlitePool;

#[derive(Clone)]
pub struct SqliteConfigStore {
    pool: SqlitePool,
}

impl SqliteConfigStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

/// Parse a stored JSON string into a [`Value`], mapping a decode failure to a
/// database error since the column is written only by [`SqliteConfigStore::set`].
fn parse_value(text: &str) -> Result<Value, DomainError> {
    serde_json::from_str(text)
        .map_err(|e| DomainError::Database(format!("config value is not valid JSON: {e}")))
}

#[async_trait]
impl ConfigStore for SqliteConfigStore {
    async fn get(&self, key: &str) -> Result<Option<Value>, DomainError> {
        let row = sqlx::query("SELECT value FROM sbh_app_config WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        match row {
            Some(r) => Ok(Some(parse_value(
                &r.try_get::<String, _>("value").map_err(db_err)?,
            )?)),
            None => Ok(None),
        }
    }

    async fn set(&self, key: &str, value: &Value) -> Result<(), DomainError> {
        let text = serde_json::to_string(value)?;
        sqlx::query(
            "INSERT INTO sbh_app_config (key, value, updated_at) VALUES (?, ?, ?) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(text)
        .bind(Utc::now())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_all(&self) -> Result<Vec<ConfigEntry>, DomainError> {
        let rows = sqlx::query("SELECT key, value, updated_at FROM sbh_app_config")
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(ConfigEntry {
                key: r.try_get("key").map_err(db_err)?,
                value: parse_value(&r.try_get::<String, _>("value").map_err(db_err)?)?,
                updated_at: r
                    .try_get::<DateTime<Utc>, _>("updated_at")
                    .map_err(db_err)?,
            });
        }
        Ok(out)
    }

    async fn delete(&self, key: &str) -> Result<(), DomainError> {
        sqlx::query("DELETE FROM sbh_app_config WHERE key = ?")
            .bind(key)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
