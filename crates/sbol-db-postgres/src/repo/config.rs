//! Durable instance configuration over Postgres.
//!
//! Entries live in `sbh_app_config`, a flat key to `jsonb` value table.
//! [`set`](ConfigStore::set) upserts on the primary key, so a later write to a
//! key overwrites its value and refreshes `updated_at`.

use async_trait::async_trait;
use sbol_db_core::{ConfigEntry, DomainError};
use sbol_db_storage::ConfigStore;
use serde_json::Value;
use sqlx::Row;

use crate::repo::db_err;
use crate::PgPool;

#[derive(Clone)]
pub struct PgConfigStore {
    pool: PgPool,
}

impl PgConfigStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ConfigStore for PgConfigStore {
    async fn get(&self, key: &str) -> Result<Option<Value>, DomainError> {
        let row = sqlx::query("SELECT value FROM sbh_app_config WHERE key = $1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        match row {
            Some(r) => Ok(Some(r.try_get::<Value, _>("value").map_err(db_err)?)),
            None => Ok(None),
        }
    }

    async fn set(&self, key: &str, value: &Value) -> Result<(), DomainError> {
        sqlx::query(
            "INSERT INTO sbh_app_config (key, value, updated_at) VALUES ($1, $2, now()) \
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
        )
        .bind(key)
        .bind(value)
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
                value: r.try_get("value").map_err(db_err)?,
                updated_at: r.try_get("updated_at").map_err(db_err)?,
            });
        }
        Ok(out)
    }

    async fn delete(&self, key: &str) -> Result<(), DomainError> {
        sqlx::query("DELETE FROM sbh_app_config WHERE key = $1")
            .bind(key)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
