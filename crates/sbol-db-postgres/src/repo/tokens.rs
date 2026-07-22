//! The API-token store over Postgres.
//!
//! Tokens are persisted only as their hash (the application layer hashes the
//! plaintext), so a leaked row cannot be replayed.

use async_trait::async_trait;
use sbol_db_core::{DomainError, UserId};
use sbol_db_storage::TokenStore;
use sqlx::Row;
use uuid::Uuid;

use crate::repo::db_err;
use crate::PgPool;

#[derive(Clone)]
pub struct PgTokenStore {
    pool: PgPool,
}

impl PgTokenStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl TokenStore for PgTokenStore {
    async fn issue(&self, token_hash: &str, user_id: UserId) -> Result<(), DomainError> {
        sqlx::query("INSERT INTO sbh_api_token (token_hash, user_id) VALUES ($1, $2)")
            .bind(token_hash)
            .bind(user_id.as_uuid())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn resolve(&self, token_hash: &str) -> Result<Option<UserId>, DomainError> {
        let row = sqlx::query("SELECT user_id FROM sbh_api_token WHERE token_hash = $1")
            .bind(token_hash)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        match row {
            Some(r) => {
                let user_id: Uuid = r.try_get("user_id").map_err(db_err)?;
                Ok(Some(UserId(user_id)))
            }
            None => Ok(None),
        }
    }

    async fn revoke(&self, token_hash: &str) -> Result<bool, DomainError> {
        let res = sqlx::query("DELETE FROM sbh_api_token WHERE token_hash = $1")
            .bind(token_hash)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(res.rows_affected() == 1)
    }
}
