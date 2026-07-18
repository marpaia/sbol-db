//! The MinHash/LSH similarity sketch store over RocksDB.
//!
//! Three column families hold the index. `seq_sketch` maps an IRI to its
//! signature bytes. `seq_lsh_band` keys `band_hash(8, BE) ++ iri -> ()` so a
//! band's members are one prefix scan (the candidate-generation query).
//! `seq_lsh_band_by_iri` mirrors `iri ++ SEP ++ band_hash(8, BE) -> ()` so a
//! re-index drops a sequence's old bands, the same mirror pattern the k-mer
//! seed index uses. A per-sequence write replaces all three in one
//! [`WriteBatch`], so a reader never sees a sequence's stale bands.

use std::collections::HashSet;

use async_trait::async_trait;
use rocksdb::WriteBatch;
use sbol_db_core::DomainError;
use sbol_db_storage::{Signature, SketchStore};

use crate::db::{Db, SEP};

const CF_SKETCH: &str = "seq_sketch";
const CF_BAND: &str = "seq_lsh_band";
const CF_BAND_BY_IRI: &str = "seq_lsh_band_by_iri";

#[derive(Clone)]
pub struct RocksdbSketchStore {
    db: Db,
}

impl RocksdbSketchStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

fn decode_sig(bytes: &[u8]) -> Result<Signature, DomainError> {
    Signature::from_bytes(bytes).ok_or_else(|| {
        DomainError::Database("stored sketch signature has an invalid length".into())
    })
}

/// `band_hash (8 bytes, BE) ++ iri` — the fixed-width band prefix for scans.
fn band_key(band: u64, iri: &str) -> Vec<u8> {
    let mut key = band.to_be_bytes().to_vec();
    key.extend_from_slice(iri.as_bytes());
    key
}

/// `iri ++ SEP ++ band_hash (8 bytes, BE)` — the per-sequence mirror.
fn by_iri_key(iri: &str, band: u64) -> Vec<u8> {
    let mut key = iri.as_bytes().to_vec();
    key.push(SEP);
    key.extend_from_slice(&band.to_be_bytes());
    key
}

#[async_trait]
impl SketchStore for RocksdbSketchStore {
    async fn put_sketch(
        &self,
        iri: &str,
        signature: &Signature,
        bands: &[u64],
    ) -> Result<(), DomainError> {
        let db = self.db.clone();
        let iri = iri.to_owned();
        let sig_bytes = signature.to_bytes();
        let bands = bands.to_vec();
        blocking(move || {
            // Gather the sequence's existing band hashes from the mirror so the
            // old postings can be deleted before the new ones land.
            let mut prefix = iri.as_bytes().to_vec();
            prefix.push(SEP);
            let mut old_bands: Vec<u64> = Vec::new();
            db.for_each_prefix(CF_BAND_BY_IRI, &prefix, |key, _| {
                let tail = &key[prefix.len()..];
                let arr: [u8; 8] = tail
                    .try_into()
                    .map_err(|_| DomainError::Database("lsh band key is not 8 bytes".into()))?;
                old_bands.push(u64::from_be_bytes(arr));
                Ok(true)
            })?;

            let cf_sketch = db.cf(CF_SKETCH);
            let cf_band = db.cf(CF_BAND);
            let cf_by_iri = db.cf(CF_BAND_BY_IRI);
            let mut batch = WriteBatch::default();
            for band in old_bands {
                batch.delete_cf(&cf_band, band_key(band, &iri));
                batch.delete_cf(&cf_by_iri, by_iri_key(&iri, band));
            }
            batch.put_cf(&cf_sketch, iri.as_bytes(), &sig_bytes);
            for band in bands {
                batch.put_cf(&cf_band, band_key(band, &iri), []);
                batch.put_cf(&cf_by_iri, by_iri_key(&iri, band), []);
            }
            db.write(batch)
        })
        .await
    }

    async fn sketch_of(&self, iri: &str) -> Result<Option<Signature>, DomainError> {
        let db = self.db.clone();
        let iri = iri.to_owned();
        blocking(move || match db.get_cf(CF_SKETCH, iri.as_bytes())? {
            Some(bytes) => Ok(Some(decode_sig(&bytes)?)),
            None => Ok(None),
        })
        .await
    }

    async fn candidates_by_bands(&self, bands: &[u64]) -> Result<Vec<String>, DomainError> {
        let db = self.db.clone();
        let bands = bands.to_vec();
        blocking(move || {
            let mut out: Vec<String> = Vec::new();
            let mut seen: HashSet<String> = HashSet::new();
            for band in bands {
                let prefix = band.to_be_bytes();
                db.for_each_prefix(CF_BAND, &prefix, |key, _| {
                    let iri = std::str::from_utf8(&key[prefix.len()..])
                        .map_err(|_| DomainError::Database("non-utf8 sequence iri".into()))?;
                    if seen.insert(iri.to_owned()) {
                        out.push(iri.to_owned());
                    }
                    Ok(true)
                })?;
            }
            Ok(out)
        })
        .await
    }

    async fn all_sketches(&self) -> Result<Vec<(String, Signature)>, DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut out = Vec::new();
            db.for_each(CF_SKETCH, |key, value| {
                let iri = String::from_utf8(key.to_vec())
                    .map_err(|_| DomainError::Database("non-utf8 sequence iri".into()))?;
                out.push((iri, decode_sig(value)?));
                Ok(true)
            })?;
            Ok(out)
        })
        .await
    }

    async fn replace_all_sketches(
        &self,
        entries: Vec<(String, Signature, Vec<u64>)>,
    ) -> Result<(), DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut existing = Vec::new();
            db.for_each(CF_SKETCH, |key, _| {
                existing.push(key.to_owned());
                Ok(true)
            })?;
            let mut existing_band = Vec::new();
            db.for_each(CF_BAND, |key, _| {
                existing_band.push(key.to_owned());
                Ok(true)
            })?;
            let mut existing_by_iri = Vec::new();
            db.for_each(CF_BAND_BY_IRI, |key, _| {
                existing_by_iri.push(key.to_owned());
                Ok(true)
            })?;

            let cf_sketch = db.cf(CF_SKETCH);
            let cf_band = db.cf(CF_BAND);
            let cf_by_iri = db.cf(CF_BAND_BY_IRI);
            let mut batch = WriteBatch::default();
            for key in existing {
                batch.delete_cf(&cf_sketch, key);
            }
            for key in existing_band {
                batch.delete_cf(&cf_band, key);
            }
            for key in existing_by_iri {
                batch.delete_cf(&cf_by_iri, key);
            }
            for (iri, signature, bands) in &entries {
                batch.put_cf(&cf_sketch, iri.as_bytes(), signature.to_bytes());
                for band in bands {
                    batch.put_cf(&cf_band, band_key(*band, iri), []);
                    batch.put_cf(&cf_by_iri, by_iri_key(iri, *band), []);
                }
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
