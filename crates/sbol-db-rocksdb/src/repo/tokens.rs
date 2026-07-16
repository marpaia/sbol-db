//! The API-token store over RocksDB.
//!
//! The `api_tokens` column family maps a token hash to the account it
//! authenticates. Tokens are persisted only as their hash (the application
//! layer hashes the plaintext), so a leaked row cannot be replayed.

use async_trait::async_trait;
use sbol_db_core::{DomainError, UserId};
use sbol_db_storage::TokenStore;
use uuid::Uuid;

use crate::db::Db;

const CF_TOKENS: &str = "api_tokens";

#[derive(Clone)]
pub struct RocksdbTokenStore {
    db: Db,
}

impl RocksdbTokenStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl TokenStore for RocksdbTokenStore {
    async fn issue(&self, token_hash: &str, user_id: UserId) -> Result<(), DomainError> {
        let db = self.db.clone();
        let token_hash = token_hash.to_owned();
        blocking(move || {
            db.put_cf(
                CF_TOKENS,
                token_hash.as_bytes(),
                user_id.as_uuid().as_bytes(),
            )
        })
        .await
    }

    async fn resolve(&self, token_hash: &str) -> Result<Option<UserId>, DomainError> {
        let db = self.db.clone();
        let token_hash = token_hash.to_owned();
        blocking(move || match db.get_cf(CF_TOKENS, token_hash.as_bytes())? {
            Some(bytes) => {
                let uuid = Uuid::from_slice(&bytes)
                    .map_err(|_| DomainError::Database("bad user id".into()))?;
                Ok(Some(UserId(uuid)))
            }
            None => Ok(None),
        })
        .await
    }

    async fn revoke(&self, token_hash: &str) -> Result<bool, DomainError> {
        let db = self.db.clone();
        let token_hash = token_hash.to_owned();
        blocking(move || {
            if !db.exists_cf(CF_TOKENS, token_hash.as_bytes())? {
                return Ok(false);
            }
            db.delete_cf(CF_TOKENS, token_hash.as_bytes())?;
            Ok(true)
        })
        .await
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
