//! The object PageRank store over Postgres.
//!
//! Scores live in `object_pagerank`. A rebuild replaces the whole table in one
//! transaction (`TRUNCATE` then a single `UNNEST` insert), so readers only ever
//! see a complete ranking.

use std::collections::HashMap;

use async_trait::async_trait;
use sbol_db_core::DomainError;
use sbol_db_storage::{PageRankStore, RankRow};
use sqlx::Row;

use crate::repo::db_err;
use crate::PgPool;

#[derive(Clone)]
pub struct PgPageRankStore {
    pool: PgPool,
}

impl PgPageRankStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PageRankStore for PgPageRankStore {
    async fn rank_of(&self, iri: &str) -> Result<Option<f64>, DomainError> {
        let row = sqlx::query("SELECT score FROM object_pagerank WHERE iri = $1")
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
        let rows =
            sqlx::query("SELECT iri, score FROM object_pagerank WHERE iri = ANY($1::text[])")
                .bind(iris)
                .fetch_all(&self.pool)
                .await
                .map_err(db_err)?;
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
        sqlx::query("TRUNCATE object_pagerank")
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        if !ranks.is_empty() {
            let iris: Vec<String> = ranks.iter().map(|r| r.iri.clone()).collect();
            let scores: Vec<f64> = ranks.iter().map(|r| r.score).collect();
            sqlx::query(
                r#"
                INSERT INTO object_pagerank (iri, score)
                SELECT * FROM UNNEST($1::text[], $2::float8[])
                ON CONFLICT (iri) DO UPDATE SET score = EXCLUDED.score
                "#,
            )
            .bind(&iris)
            .bind(&scores)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
}
