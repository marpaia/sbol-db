//! The MinHash/LSH similarity sketch store over Postgres.
//!
//! Signatures live in `sbol_sequence_sketch` (one BLOB per sequence) and the
//! LSH band buckets in `sbol_sequence_lsh_band` (one row per band a sequence
//! falls into). A per-sequence write replaces both in one transaction, so a
//! re-index leaves no stale postings. Candidate generation
//! ([`candidates_by_bands`](SketchStore::candidates_by_bands)) is a single
//! `band_hash = ANY(...)` posting-list union.

use async_trait::async_trait;
use sbol_db_core::DomainError;
use sbol_db_storage::{Signature, SketchStore};
use sqlx::Row;

use crate::repo::db_err;
use crate::PgPool;

#[derive(Clone)]
pub struct PgSketchStore {
    pool: PgPool,
}

impl PgSketchStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Reinterpret a 64-bit band hash as a signed `bigint` for storage. The search
/// layer only tests band_hash equality, so the sign carries no meaning.
fn to_i64(band: u64) -> i64 {
    i64::from_le_bytes(band.to_le_bytes())
}

fn decode_sig(bytes: &[u8]) -> Result<Signature, DomainError> {
    Signature::from_bytes(bytes).ok_or_else(|| {
        DomainError::Database("stored sketch signature has an invalid length".into())
    })
}

#[async_trait]
impl SketchStore for PgSketchStore {
    async fn put_sketch(
        &self,
        iri: &str,
        signature: &Signature,
        bands: &[u64],
    ) -> Result<(), DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        sqlx::query("DELETE FROM sbol_sequence_lsh_band WHERE sequence_iri = $1")
            .bind(iri)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query(
            r#"
            INSERT INTO sbol_sequence_sketch (sequence_iri, signature)
            VALUES ($1, $2)
            ON CONFLICT (sequence_iri) DO UPDATE SET signature = EXCLUDED.signature
            "#,
        )
        .bind(iri)
        .bind(signature.to_bytes())
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        if !bands.is_empty() {
            let hashes: Vec<i64> = bands.iter().map(|b| to_i64(*b)).collect();
            let iris: Vec<String> = vec![iri.to_owned(); bands.len()];
            sqlx::query(
                r#"
                INSERT INTO sbol_sequence_lsh_band (band_hash, sequence_iri)
                SELECT * FROM UNNEST($1::bigint[], $2::text[])
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(&hashes)
            .bind(&iris)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn sketch_of(&self, iri: &str) -> Result<Option<Signature>, DomainError> {
        let row = sqlx::query("SELECT signature FROM sbol_sequence_sketch WHERE sequence_iri = $1")
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
        let hashes: Vec<i64> = bands.iter().map(|b| to_i64(*b)).collect();
        let rows = sqlx::query(
            "SELECT DISTINCT sequence_iri FROM sbol_sequence_lsh_band WHERE band_hash = ANY($1::bigint[])",
        )
        .bind(&hashes)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
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

        let mut sketch_iris: Vec<String> = Vec::with_capacity(entries.len());
        let mut sketch_sigs: Vec<Vec<u8>> = Vec::with_capacity(entries.len());
        let mut band_hashes: Vec<i64> = Vec::new();
        let mut band_iris: Vec<String> = Vec::new();
        for (iri, signature, bands) in &entries {
            sketch_iris.push(iri.clone());
            sketch_sigs.push(signature.to_bytes());
            for band in bands {
                band_hashes.push(to_i64(*band));
                band_iris.push(iri.clone());
            }
        }

        if !sketch_iris.is_empty() {
            sqlx::query(
                r#"
                INSERT INTO sbol_sequence_sketch (sequence_iri, signature)
                SELECT * FROM UNNEST($1::text[], $2::bytea[])
                "#,
            )
            .bind(&sketch_iris)
            .bind(&sketch_sigs)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        if !band_hashes.is_empty() {
            sqlx::query(
                r#"
                INSERT INTO sbol_sequence_lsh_band (band_hash, sequence_iri)
                SELECT * FROM UNNEST($1::bigint[], $2::text[])
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(&band_hashes)
            .bind(&band_iris)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
}
