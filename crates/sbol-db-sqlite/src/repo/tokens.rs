//! The API-token store over SQLite.
//!
//! Tokens are persisted only as their hash (the application layer hashes the
//! plaintext), so a leaked row cannot be replayed.

use async_trait::async_trait;
use chrono::Utc;
use sbol_db_core::{DomainError, UserId};
use sbol_db_storage::TokenStore;
use sqlx::Row;

use crate::pool::db_err;
use crate::SqlitePool;

#[derive(Clone)]
pub struct SqliteTokenStore {
    pool: SqlitePool,
}

impl SqliteTokenStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl TokenStore for SqliteTokenStore {
    async fn issue(&self, token_hash: &str, user_id: UserId) -> Result<(), DomainError> {
        sqlx::query("INSERT INTO sbh_api_token (token_hash, user_id, created_at) VALUES (?, ?, ?)")
            .bind(token_hash)
            .bind(user_id.as_uuid().to_string())
            .bind(Utc::now())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn resolve(&self, token_hash: &str) -> Result<Option<UserId>, DomainError> {
        let row = sqlx::query("SELECT user_id FROM sbh_api_token WHERE token_hash = ?")
            .bind(token_hash)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        match row {
            Some(r) => {
                let user_id: String = r.try_get("user_id").map_err(db_err)?;
                Ok(Some(UserId(
                    uuid::Uuid::parse_str(&user_id).map_err(db_err)?,
                )))
            }
            None => Ok(None),
        }
    }

    async fn revoke(&self, token_hash: &str) -> Result<bool, DomainError> {
        let res = sqlx::query("DELETE FROM sbh_api_token WHERE token_hash = ?")
            .bind(token_hash)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(res.rows_affected() == 1)
    }
}
