//! The object PageRank store over SQLite.
//!
//! Scores live in `object_pagerank`. A rebuild replaces the whole table in one
//! transaction (`DELETE` then row-by-row insert), so readers only ever see a
//! complete ranking.

use std::collections::HashMap;

use async_trait::async_trait;
use sbol_db_core::DomainError;
use sbol_db_storage::{PageRankStore, RankRow};
use sqlx::Row;

use crate::pool::db_err;
use crate::SqlitePool;

#[derive(Clone)]
pub struct SqlitePageRankStore {
    pool: SqlitePool,
}

impl SqlitePageRankStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PageRankStore for SqlitePageRankStore {
    async fn rank_of(&self, iri: &str) -> Result<Option<f64>, DomainError> {
        let row = sqlx::query("SELECT score FROM object_pagerank WHERE iri = ?")
            .bind(iri)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        match row {
            Some(r) => Ok(Some(r.try_get::<f64, _>("score").map_err(db_err)?)),
            None => Ok(None),
        }
    }

    async fn ranks_for(&self, iris: &[String]) -> Result<HashMap<String, f64>, DomainError> {
        if iris.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = vec!["?"; iris.len()].join(", ");
        let sql = format!("SELECT iri, score FROM object_pagerank WHERE iri IN ({placeholders})");
        let mut q = sqlx::query(&sql);
        for iri in iris {
            q = q.bind(iri);
        }
        let rows = q.fetch_all(&self.pool).await.map_err(db_err)?;
        let mut out = HashMap::with_capacity(rows.len());
        for r in rows {
            let iri: String = r.try_get("iri").map_err(db_err)?;
            let score: f64 = r.try_get("score").map_err(db_err)?;
            out.insert(iri, score);
        }
        Ok(out)
    }

    async fn all_ranks(&self) -> Result<Vec<RankRow>, DomainError> {
        let rows = sqlx::query("SELECT iri, score FROM object_pagerank")
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let iri: String = r.try_get("iri").map_err(db_err)?;
            let score: f64 = r.try_get("score").map_err(db_err)?;
            out.push(RankRow { iri, score });
        }
        Ok(out)
    }

    async fn replace_all_ranks(&self, ranks: Vec<RankRow>) -> Result<(), DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        sqlx::query("DELETE FROM object_pagerank")
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        for row in &ranks {
            sqlx::query(
                "INSERT INTO object_pagerank (iri, score) VALUES (?, ?) \
                 ON CONFLICT (iri) DO UPDATE SET score = excluded.score",
            )
            .bind(&row.iri)
            .bind(row.score)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
}
