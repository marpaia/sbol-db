//! Durable one-time prepared mutations over Postgres.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sbol_db_core::{DomainError, PreparedMutation, UserId};
use sbol_db_storage::PreparedMutationStore;
use sqlx::postgres::PgRow;
use sqlx::Row;
use uuid::Uuid;

use crate::repo::db_err;
use crate::PgPool;

#[derive(Clone)]
pub struct PgPreparedMutationStore {
    pool: PgPool,
}

impl PgPreparedMutationStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PreparedMutationStore for PgPreparedMutationStore {
    async fn put_prepared_mutation(&self, plan: PreparedMutation) -> Result<(), DomainError> {
        sqlx::query(
            "INSERT INTO sbol_prepared_mutation \
             (token_hash, user_id, oauth_client_id, audience, required_scopes, operation, \
              target_iri, expected_content_etag, input_hash, effect, payload, created_at, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        )
        .bind(plan.token_hash)
        .bind(plan.user_id.as_uuid())
        .bind(plan.oauth_client_id)
        .bind(plan.audience)
        .bind(serde_json::to_value(plan.required_scopes)?)
        .bind(plan.operation)
        .bind(plan.target_iri)
        .bind(plan.expected_content_etag)
        .bind(plan.input_hash)
        .bind(plan.effect)
        .bind(plan.payload)
        .bind(plan.created_at)
        .bind(plan.expires_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_prepared_mutation(
        &self,
        token_hash: &str,
    ) -> Result<Option<PreparedMutation>, DomainError> {
        sqlx::query(SELECT_PLAN)
            .bind(token_hash)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?
            .map(plan_from_row)
            .transpose()
    }

    async fn consume_prepared_mutation(
        &self,
        token_hash: &str,
    ) -> Result<Option<PreparedMutation>, DomainError> {
        sqlx::query(
            "DELETE FROM sbol_prepared_mutation WHERE token_hash = $1 RETURNING \
             token_hash, user_id, oauth_client_id, audience, required_scopes, operation, \
             target_iri, expected_content_etag, input_hash, effect, payload, created_at, expires_at",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?
        .map(plan_from_row)
        .transpose()
    }

    async fn purge_expired_prepared_mutations(
        &self,
        now: DateTime<Utc>,
    ) -> Result<usize, DomainError> {
        let result = sqlx::query("DELETE FROM sbol_prepared_mutation WHERE expires_at <= $1")
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(result.rows_affected() as usize)
    }
}

const SELECT_PLAN: &str =
    "SELECT token_hash, user_id, oauth_client_id, audience, required_scopes, operation, \
     target_iri, expected_content_etag, input_hash, effect, payload, created_at, expires_at \
     FROM sbol_prepared_mutation WHERE token_hash = $1";

fn plan_from_row(row: PgRow) -> Result<PreparedMutation, DomainError> {
    let user_id: Uuid = row.try_get("user_id").map_err(db_err)?;
    let required_scopes: serde_json::Value = row.try_get("required_scopes").map_err(db_err)?;
    Ok(PreparedMutation {
        token_hash: row.try_get("token_hash").map_err(db_err)?,
        user_id: UserId(user_id),
        oauth_client_id: row.try_get("oauth_client_id").map_err(db_err)?,
        audience: row.try_get("audience").map_err(db_err)?,
        required_scopes: serde_json::from_value(required_scopes)?,
        operation: row.try_get("operation").map_err(db_err)?,
        target_iri: row.try_get("target_iri").map_err(db_err)?,
        expected_content_etag: row.try_get("expected_content_etag").map_err(db_err)?,
        input_hash: row.try_get("input_hash").map_err(db_err)?,
        effect: row.try_get("effect").map_err(db_err)?,
        payload: row.try_get("payload").map_err(db_err)?,
        created_at: row.try_get("created_at").map_err(db_err)?,
        expires_at: row.try_get("expires_at").map_err(db_err)?,
    })
}
