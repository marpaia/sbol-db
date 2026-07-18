//! The sequence cluster store over SQLite.
//!
//! Assignments live in `sbol_sequence_cluster`. A rebuild replaces the whole
//! table in one transaction (`DELETE` then row-by-row insert), so readers only
//! ever see a complete clustering.

use async_trait::async_trait;
use sbol_db_core::DomainError;
use sbol_db_storage::{ClusterId, ClusterStore};
use sqlx::Row;

use crate::pool::db_err;
use crate::SqlitePool;

#[derive(Clone)]
pub struct SqliteClusterStore {
    pool: SqlitePool,
}

impl SqliteClusterStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ClusterStore for SqliteClusterStore {
    async fn cluster_id_of(&self, iri: &str) -> Result<Option<ClusterId>, DomainError> {
        let row =
            sqlx::query("SELECT cluster_id FROM sbol_sequence_cluster WHERE sequence_iri = ?")
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
            WHERE self_row.sequence_iri = ?
              AND mate.sequence_iri <> ?
            "#,
        )
        .bind(iri)
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
        sqlx::query("DELETE FROM sbol_sequence_cluster")
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        for (iri, cluster_id) in &pairs {
            sqlx::query(
                "INSERT INTO sbol_sequence_cluster (sequence_iri, cluster_id) VALUES (?, ?) \
                 ON CONFLICT (sequence_iri) DO UPDATE SET cluster_id = excluded.cluster_id",
            )
            .bind(iri)
            .bind(cluster_id.0)
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

    async fn assign_cluster(&self, iri: &str, cluster: ClusterId) -> Result<(), DomainError> {
        sqlx::query(
            "INSERT INTO sbol_sequence_cluster (sequence_iri, cluster_id) VALUES (?, ?) \
             ON CONFLICT (sequence_iri) DO UPDATE SET cluster_id = excluded.cluster_id",
        )
        .bind(iri)
        .bind(cluster.0)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn max_cluster_id(&self) -> Result<Option<ClusterId>, DomainError> {
        let row = sqlx::query("SELECT MAX(cluster_id) AS max_id FROM sbol_sequence_cluster")
            .fetch_one(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(row
            .try_get::<Option<i64>, _>("max_id")
            .map_err(db_err)?
            .map(ClusterId))
    }
}
