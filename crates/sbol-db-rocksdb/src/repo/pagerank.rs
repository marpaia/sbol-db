//! The object PageRank store over RocksDB.
//!
//! The `object_pagerank` column family maps an object IRI to its score, encoded
//! as a little-endian `f64`. A rebuild replaces the whole family in one
//! [`WriteBatch`] (delete every existing key, then write the new scores), so a
//! reader only ever sees a complete ranking.

use std::collections::HashMap;

use async_trait::async_trait;
use rocksdb::WriteBatch;
use sbol_db_core::DomainError;
use sbol_db_storage::{PageRankStore, RankRow};

use crate::db::Db;

const CF_PAGERANK: &str = "object_pagerank";

#[derive(Clone)]
pub struct RocksdbPageRankStore {
    db: Db,
}

impl RocksdbPageRankStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

/// Decode a stored little-endian `f64` score, erroring on a wrong-width value.
fn decode_score(bytes: &[u8]) -> Result<f64, DomainError> {
    let arr: [u8; 8] = bytes
        .try_into()
        .map_err(|_| DomainError::Database("pagerank score is not an f64".into()))?;
    Ok(f64::from_le_bytes(arr))
}

#[async_trait]
impl PageRankStore for RocksdbPageRankStore {
    async fn rank_of(&self, iri: &str) -> Result<Option<f64>, DomainError> {
        let db = self.db.clone();
        let iri = iri.to_owned();
        blocking(move || match db.get_cf(CF_PAGERANK, iri.as_bytes())? {
            Some(bytes) => Ok(Some(decode_score(&bytes)?)),
            None => Ok(None),
        })
        .await
    }

    async fn ranks_for(&self, iris: &[String]) -> Result<HashMap<String, f64>, DomainError> {
        let db = self.db.clone();
        let iris = iris.to_vec();
        blocking(move || {
            let mut out = HashMap::with_capacity(iris.len());
            for iri in iris {
                if let Some(bytes) = db.get_cf(CF_PAGERANK, iri.as_bytes())? {
                    out.insert(iri, decode_score(&bytes)?);
                }
            }
            Ok(out)
        })
        .await
    }

    async fn all_ranks(&self) -> Result<Vec<RankRow>, DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut out = Vec::new();
            db.for_each(CF_PAGERANK, |key, value| {
                let iri = String::from_utf8(key.to_owned())
                    .map_err(|_| DomainError::Database("pagerank iri is not utf-8".into()))?;
                out.push(RankRow {
                    iri,
                    score: decode_score(value)?,
                });
                Ok(true)
            })?;
            Ok(out)
        })
        .await
    }

    async fn replace_all_ranks(&self, ranks: Vec<RankRow>) -> Result<(), DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut existing = Vec::new();
            db.for_each(CF_PAGERANK, |key, _| {
                existing.push(key.to_owned());
                Ok(true)
            })?;
            let handle = db.cf(CF_PAGERANK);
            let mut batch = WriteBatch::default();
            for key in existing {
                batch.delete_cf(&handle, key);
            }
            for row in &ranks {
                batch.put_cf(&handle, row.iri.as_bytes(), row.score.to_le_bytes());
            }
            db.write(batch)
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
