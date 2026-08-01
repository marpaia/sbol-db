//! Durable SBOL Identity OAuth grants over RocksDB.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sbol_db_core::{
    DomainError, OAuthAccessToken, OAuthAuthorizationCode, OAuthClient, OAuthRefreshToken,
};
use sbol_db_storage::OAuthStore;

use crate::db::Db;

const CF_OAUTH: &str = "oauth";

#[derive(Clone)]
pub struct RocksdbOAuthStore {
    db: Db,
    consume_lock: Arc<Mutex<()>>,
}

impl RocksdbOAuthStore {
    pub fn new(db: Db) -> Self {
        let consume_lock = db.oauth_consume_lock();
        Self { db, consume_lock }
    }
}

#[async_trait]
impl OAuthStore for RocksdbOAuthStore {
    async fn register_client(&self, client: OAuthClient) -> Result<(), DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let key = key("client", &client.client_id);
            if db.exists_cf(CF_OAUTH, &key)? {
                return Err(DomainError::InvalidInput(
                    "OAuth client id is already registered".to_owned(),
                ));
            }
            db.put_cf(CF_OAUTH, &key, &serde_json::to_vec(&client)?)
        })
        .await
    }

    async fn get_client(&self, client_id: &str) -> Result<Option<OAuthClient>, DomainError> {
        get(self.db.clone(), key("client", client_id)).await
    }

    async fn issue_authorization_code(
        &self,
        code: OAuthAuthorizationCode,
    ) -> Result<(), DomainError> {
        put(self.db.clone(), key("code", &code.code_hash), code).await
    }

    async fn consume_authorization_code(
        &self,
        code_hash: &str,
    ) -> Result<Option<OAuthAuthorizationCode>, DomainError> {
        consume(
            self.db.clone(),
            self.consume_lock.clone(),
            key("code", code_hash),
        )
        .await
    }

    async fn issue_access_token(&self, token: OAuthAccessToken) -> Result<(), DomainError> {
        put(self.db.clone(), key("access", &token.token_hash), token).await
    }

    async fn resolve_access_token(
        &self,
        token_hash: &str,
    ) -> Result<Option<OAuthAccessToken>, DomainError> {
        get(self.db.clone(), key("access", token_hash)).await
    }

    async fn revoke_access_token(&self, token_hash: &str) -> Result<bool, DomainError> {
        let db = self.db.clone();
        let key = key("access", token_hash);
        blocking(move || {
            if !db.exists_cf(CF_OAUTH, &key)? {
                return Ok(false);
            }
            db.delete_cf(CF_OAUTH, &key)?;
            Ok(true)
        })
        .await
    }

    async fn issue_refresh_token(&self, token: OAuthRefreshToken) -> Result<(), DomainError> {
        put(self.db.clone(), key("refresh", &token.token_hash), token).await
    }

    async fn consume_refresh_token(
        &self,
        token_hash: &str,
    ) -> Result<Option<OAuthRefreshToken>, DomainError> {
        consume(
            self.db.clone(),
            self.consume_lock.clone(),
            key("refresh", token_hash),
        )
        .await
    }
}

fn key(kind: &str, id: &str) -> Vec<u8> {
    let mut value = Vec::with_capacity(kind.len() + id.len() + 1);
    value.extend_from_slice(kind.as_bytes());
    value.push(0x1f);
    value.extend_from_slice(id.as_bytes());
    value
}

async fn put<T>(db: Db, key: Vec<u8>, value: T) -> Result<(), DomainError>
where
    T: serde::Serialize + Send + 'static,
{
    blocking(move || db.put_cf(CF_OAUTH, &key, &serde_json::to_vec(&value)?)).await
}

async fn get<T>(db: Db, key: Vec<u8>) -> Result<Option<T>, DomainError>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    blocking(move || {
        db.get_cf(CF_OAUTH, &key)?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(DomainError::from))
            .transpose()
    })
    .await
}

async fn consume<T>(
    db: Db,
    consume_lock: Arc<Mutex<()>>,
    key: Vec<u8>,
) -> Result<Option<T>, DomainError>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    blocking(move || {
        let _guard = consume_lock
            .lock()
            .map_err(|_| DomainError::Database("OAuth consume lock poisoned".to_owned()))?;
        let Some(bytes) = db.get_cf(CF_OAUTH, &key)? else {
            return Ok(None);
        };
        db.delete_cf(CF_OAUTH, &key)?;
        Ok(Some(serde_json::from_slice(&bytes)?))
    })
    .await
}

async fn blocking<T, F>(function: F) -> Result<T, DomainError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, DomainError> + Send + 'static,
{
    tokio::task::spawn_blocking(function)
        .await
        .map_err(|error| DomainError::Database(format!("rocksdb task panicked: {error}")))?
}
