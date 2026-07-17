//! The sequence cluster store over Postgres.
//!
//! Assignments live in `sbol_sequence_cluster`. A rebuild replaces the whole
//! table in one transaction (`TRUNCATE` then a single `UNNEST` insert), so
//! readers only ever see a complete clustering.

use async_trait::async_trait;
use sbol_db_core::DomainError;
use sbol_db_storage::{ClusterId, ClusterStore};
use sqlx::Row;

use crate::repo::db_err;
use crate::PgPool;

#[derive(Clone)]
pub struct PgClusterStore {
    pool: PgPool,
}

impl PgClusterStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ClusterStore for PgClusterStore {
    async fn cluster_id_of(&self, iri: &str) -> Result<Option<ClusterId>, DomainError> {
        let row =
            sqlx::query("SELECT cluster_id FROM sbol_sequence_cluster WHERE sequence_iri = $1")
                .bind(iri)
                .fetch_optional(&self.pool)
                .await
                .map_err(db_err)?;
        match row {
            Some(r) => Ok(Some(ClusterId(
                r.try_get::<i64, _>("cluster_id").map_err(db_err)?,
            ))),
            None => Ok(None),
        }
    }

    async fn cluster_mates(&self, iri: &str) -> Result<Vec<String>, DomainError> {
        let rows = sqlx::query(
            r#"
            SELECT mate.sequence_iri AS iri
            FROM sbol_sequence_cluster self_row
            JOIN sbol_sequence_cluster mate ON mate.cluster_id = self_row.cluster_id
            WHERE self_row.sequence_iri = $1
              AND mate.sequence_iri <> $1
            "#,
        )
        .bind(iri)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(r.try_get::<String, _>("iri").map_err(db_err)?);
        }
        Ok(out)
    }

    async fn replace_clusters(&self, pairs: Vec<(String, ClusterId)>) -> Result<(), DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        sqlx::query("TRUNCATE sbol_sequence_cluster")
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        if !pairs.is_empty() {
            let iris: Vec<String> = pairs.iter().map(|(iri, _)| iri.clone()).collect();
            let cluster_ids: Vec<i64> = pairs.iter().map(|(_, c)| c.0).collect();
            sqlx::query(
                r#"
                INSERT INTO sbol_sequence_cluster (sequence_iri, cluster_id)
                SELECT * FROM UNNEST($1::text[], $2::bigint[])
                ON CONFLICT (sequence_iri) DO UPDATE SET cluster_id = EXCLUDED.cluster_id
                "#,
            )
            .bind(&iris)
            .bind(&cluster_ids)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn all_assignments(&self) -> Result<Vec<(String, ClusterId)>, DomainError> {
        let rows = sqlx::query("SELECT sequence_iri, cluster_id FROM sbol_sequence_cluster")
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let iri = r.try_get::<String, _>("sequence_iri").map_err(db_err)?;
            let cluster = ClusterId(r.try_get::<i64, _>("cluster_id").map_err(db_err)?);
            out.push((iri, cluster));
        }
        Ok(out)
    }
}
