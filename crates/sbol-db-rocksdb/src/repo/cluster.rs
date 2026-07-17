//! The sequence cluster store over RocksDB.
//!
//! Two column families hold the clustering. `sequence_cluster` maps an IRI to
//! its cluster id (a big-endian `i64`); `sequence_cluster_by_id` keys
//! `cluster_id(BE) ++ SEP ++ iri -> ()` so a cluster's members are one prefix
//! scan. A rebuild replaces both families in one [`WriteBatch`] (delete every
//! existing key, then write the new assignments), so a reader only ever sees a
//! complete clustering.

use async_trait::async_trait;
use rocksdb::WriteBatch;
use sbol_db_core::DomainError;
use sbol_db_storage::{ClusterId, ClusterStore};

use crate::db::{compose, Db, SEP};

const CF_CLUSTER: &str = "sequence_cluster";
const CF_CLUSTER_BY_ID: &str = "sequence_cluster_by_id";

#[derive(Clone)]
pub struct RocksdbClusterStore {
    db: Db,
}

impl RocksdbClusterStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

/// Decode a stored big-endian `i64` cluster id, erroring on a wrong-width value.
fn decode_id(bytes: &[u8]) -> Result<i64, DomainError> {
    let arr: [u8; 8] = bytes
        .try_into()
        .map_err(|_| DomainError::Database("cluster id is not an i64".into()))?;
    Ok(i64::from_be_bytes(arr))
}

#[async_trait]
impl ClusterStore for RocksdbClusterStore {
    async fn cluster_id_of(&self, iri: &str) -> Result<Option<ClusterId>, DomainError> {
        let db = self.db.clone();
        let iri = iri.to_owned();
        blocking(move || match db.get_cf(CF_CLUSTER, iri.as_bytes())? {
            Some(bytes) => Ok(Some(ClusterId(decode_id(&bytes)?))),
            None => Ok(None),
        })
        .await
    }

    async fn cluster_mates(&self, iri: &str) -> Result<Vec<String>, DomainError> {
        let db = self.db.clone();
        let iri = iri.to_owned();
        blocking(move || {
            let Some(bytes) = db.get_cf(CF_CLUSTER, iri.as_bytes())? else {
                return Ok(Vec::new());
            };
            let id = decode_id(&bytes)?;
            let mut prefix = id.to_be_bytes().to_vec();
            prefix.push(SEP);
            let mut out = Vec::new();
            db.for_each_prefix(CF_CLUSTER_BY_ID, &prefix, |key, _| {
                let mate = String::from_utf8(key[prefix.len()..].to_vec())
                    .map_err(|_| DomainError::Database("cluster member iri is not utf-8".into()))?;
                if mate != iri {
                    out.push(mate);
                }
                Ok(true)
            })?;
            Ok(out)
        })
        .await
    }

    async fn replace_clusters(&self, pairs: Vec<(String, ClusterId)>) -> Result<(), DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut existing = Vec::new();
            db.for_each(CF_CLUSTER, |key, _| {
                existing.push(key.to_owned());
                Ok(true)
            })?;
            let mut existing_by_id = Vec::new();
            db.for_each(CF_CLUSTER_BY_ID, |key, _| {
                existing_by_id.push(key.to_owned());
                Ok(true)
            })?;

            let cf = db.cf(CF_CLUSTER);
            let cf_by_id = db.cf(CF_CLUSTER_BY_ID);
            let mut batch = WriteBatch::default();
            for key in existing {
                batch.delete_cf(&cf, key);
            }
            for key in existing_by_id {
                batch.delete_cf(&cf_by_id, key);
            }
            for (iri, cluster_id) in &pairs {
                let id_be = cluster_id.0.to_be_bytes();
                batch.put_cf(&cf, iri.as_bytes(), id_be);
                batch.put_cf(&cf_by_id, compose(&[&id_be, iri.as_bytes()]), []);
            }
            db.write(batch)
        })
        .await
    }

    async fn all_assignments(&self) -> Result<Vec<(String, ClusterId)>, DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut out = Vec::new();
            db.for_each(CF_CLUSTER, |key, value| {
                let iri = String::from_utf8(key.to_owned())
                    .map_err(|_| DomainError::Database("cluster member iri is not utf-8".into()))?;
                out.push((iri, ClusterId(decode_id(value)?)));
                Ok(true)
            })?;
            Ok(out)
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
