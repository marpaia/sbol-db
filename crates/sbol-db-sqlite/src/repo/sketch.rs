//! The MinHash/LSH similarity sketch store over SQLite.
//!
//! Signatures live in `sbol_sequence_sketch` (one BLOB per sequence) and the
//! LSH band buckets in `sbol_sequence_lsh_band` (one row per band a sequence
//! falls into). A per-sequence write replaces both in one transaction, so a
//! re-index leaves no stale postings. Candidate generation
//! ([`candidates_by_bands`](SketchStore::candidates_by_bands)) is a posting-list
//! union over `band_hash`.

use async_trait::async_trait;
use sbol_db_core::DomainError;
use sbol_db_storage::{Signature, SketchStore};
use sqlx::Row;

use crate::pool::db_err;
use crate::SqlitePool;

#[derive(Clone)]
pub struct SqliteSketchStore {
    pool: SqlitePool,
}

impl SqliteSketchStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

/// Reinterpret a 64-bit band hash as a signed integer for the INTEGER column.
/// The search layer only tests band_hash equality, so the sign carries no
/// meaning.
fn to_i64(band: u64) -> i64 {
    i64::from_le_bytes(band.to_le_bytes())
}

fn decode_sig(bytes: &[u8]) -> Result<Signature, DomainError> {
    Signature::from_bytes(bytes).ok_or_else(|| {
        DomainError::Database("stored sketch signature has an invalid length".into())
    })
}

#[async_trait]
impl SketchStore for SqliteSketchStore {
    async fn put_sketch(
        &self,
        iri: &str,
        signature: &Signature,
        bands: &[u64],
    ) -> Result<(), DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        sqlx::query("DELETE FROM sbol_sequence_lsh_band WHERE sequence_iri = ?")
            .bind(iri)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query(
            "INSERT INTO sbol_sequence_sketch (sequence_iri, signature) VALUES (?, ?) \
             ON CONFLICT (sequence_iri) DO UPDATE SET signature = excluded.signature",
        )
        .bind(iri)
        .bind(signature.to_bytes())
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        for band in bands {
            sqlx::query(
                "INSERT INTO sbol_sequence_lsh_band (band_hash, sequence_iri) VALUES (?, ?) \
                 ON CONFLICT DO NOTHING",
            )
            .bind(to_i64(*band))
            .bind(iri)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn sketch_of(&self, iri: &str) -> Result<Option<Signature>, DomainError> {
        let row = sqlx::query("SELECT signature FROM sbol_sequence_sketch WHERE sequence_iri = ?")
            .bind(iri)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        match row {
            Some(r) => {
                let bytes: Vec<u8> = r.try_get("signature").map_err(db_err)?;
                Ok(Some(decode_sig(&bytes)?))
            }
            None => Ok(None),
        }
    }

    async fn candidates_by_bands(&self, bands: &[u64]) -> Result<Vec<String>, DomainError> {
        if bands.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; bands.len()].join(", ");
        let sql = format!(
            "SELECT DISTINCT sequence_iri FROM sbol_sequence_lsh_band \
             WHERE band_hash IN ({placeholders})"
        );
        let mut q = sqlx::query(&sql);
        for band in bands {
            q = q.bind(to_i64(*band));
        }
        let rows = q.fetch_all(&self.pool).await.map_err(db_err)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(r.try_get::<String, _>("sequence_iri").map_err(db_err)?);
        }
        Ok(out)
    }

    async fn all_sketches(&self) -> Result<Vec<(String, Signature)>, DomainError> {
        let rows = sqlx::query("SELECT sequence_iri, signature FROM sbol_sequence_sketch")
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let iri: String = r.try_get("sequence_iri").map_err(db_err)?;
            let bytes: Vec<u8> = r.try_get("signature").map_err(db_err)?;
            out.push((iri, decode_sig(&bytes)?));
        }
        Ok(out)
    }

    async fn replace_all_sketches(
        &self,
        entries: Vec<(String, Signature, Vec<u64>)>,
    ) -> Result<(), DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        sqlx::query("DELETE FROM sbol_sequence_lsh_band")
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query("DELETE FROM sbol_sequence_sketch")
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        for (iri, signature, bands) in &entries {
            sqlx::query("INSERT INTO sbol_sequence_sketch (sequence_iri, signature) VALUES (?, ?)")
                .bind(iri)
                .bind(signature.to_bytes())
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            for band in bands {
                sqlx::query(
                    "INSERT INTO sbol_sequence_lsh_band (band_hash, sequence_iri) VALUES (?, ?) \
                     ON CONFLICT DO NOTHING",
                )
                .bind(to_i64(*band))
                .bind(iri)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
}
