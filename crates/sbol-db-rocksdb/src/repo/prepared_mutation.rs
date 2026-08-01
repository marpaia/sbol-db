//! Durable one-time prepared mutations over RocksDB.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rocksdb::WriteBatch;
use sbol_db_core::{DomainError, PreparedMutation};
use sbol_db_storage::PreparedMutationStore;

use crate::db::Db;

const CF: &str = "prepared_mutation";

#[derive(Clone)]
pub struct RocksdbPreparedMutationStore {
    db: Db,
    consume_lock: Arc<Mutex<()>>,
}

impl RocksdbPreparedMutationStore {
    pub fn new(db: Db) -> Self {
        let consume_lock = db.prepared_mutation_lock();
        Self { db, consume_lock }
    }
}

#[async_trait]
impl PreparedMutationStore for RocksdbPreparedMutationStore {
    async fn put_prepared_mutation(&self, plan: PreparedMutation) -> Result<(), DomainError> {
        let db = self.db.clone();
        blocking(move || {
            if db.exists_cf(CF, plan.token_hash.as_bytes())? {
                return Err(DomainError::InvalidInput(
                    "prepared mutation token already exists".to_owned(),
                ));
            }
            db.put_cf(CF, plan.token_hash.as_bytes(), &serde_json::to_vec(&plan)?)
        })
        .await
    }

    async fn get_prepared_mutation(
        &self,
        token_hash: &str,
    ) -> Result<Option<PreparedMutation>, DomainError> {
        let db = self.db.clone();
        let key = token_hash.as_bytes().to_vec();
        blocking(move || {
            db.get_cf(CF, &key)?
                .map(|bytes| serde_json::from_slice(&bytes).map_err(DomainError::from))
                .transpose()
        })
        .await
    }

    async fn consume_prepared_mutation(
        &self,
        token_hash: &str,
    ) -> Result<Option<PreparedMutation>, DomainError> {
        let db = self.db.clone();
        let lock = self.consume_lock.clone();
        let key = token_hash.as_bytes().to_vec();
        blocking(move || {
            let _guard = lock.lock().map_err(|_| {
                DomainError::Database("prepared mutation consume lock poisoned".to_owned())
            })?;
            let Some(bytes) = db.get_cf(CF, &key)? else {
                return Ok(None);
            };
            db.delete_cf(CF, &key)?;
            Ok(Some(serde_json::from_slice(&bytes)?))
        })
        .await
    }

    async fn purge_expired_prepared_mutations(
        &self,
        now: DateTime<Utc>,
    ) -> Result<usize, DomainError> {
        let db = self.db.clone();
        let lock = self.consume_lock.clone();
        blocking(move || {
            let _guard = lock.lock().map_err(|_| {
                DomainError::Database("prepared mutation purge lock poisoned".to_owned())
            })?;
            let mut expired = Vec::new();
            db.for_each_prefix(CF, b"", |key, value| {
                let plan: PreparedMutation = serde_json::from_slice(value)?;
                if plan.expires_at <= now {
                    expired.push(key.to_vec());
                }
                Ok(true)
            })?;
            if expired.is_empty() {
                return Ok(0);
            }
            let mut batch = WriteBatch::default();
            for key in &expired {
                batch.delete_cf(&db.cf(CF), key);
            }
            db.write(batch)?;
            Ok(expired.len())
        })
        .await
    }
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
