//! Durable SBOL Identity OAuth grants over Postgres.

use async_trait::async_trait;
use sbol_db_core::{
    DomainError, OAuthAccessToken, OAuthAuthorizationCode, OAuthClient, OAuthRefreshToken, UserId,
};
use sbol_db_storage::OAuthStore;
use sqlx::postgres::PgRow;
use sqlx::Row;
use uuid::Uuid;

use crate::repo::db_err;
use crate::PgPool;

#[derive(Clone)]
pub struct PgOAuthStore {
    pool: PgPool,
}

impl PgOAuthStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl OAuthStore for PgOAuthStore {
    async fn register_client(&self, client: OAuthClient) -> Result<(), DomainError> {
        sqlx::query(
            "INSERT INTO sbol_oauth_client \
             (client_id, client_name, redirect_uris, created_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(client.client_id)
        .bind(client.client_name)
        .bind(serde_json::to_value(client.redirect_uris)?)
        .bind(client.created_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_client(&self, client_id: &str) -> Result<Option<OAuthClient>, DomainError> {
        let row = sqlx::query(
            "SELECT client_id, client_name, redirect_uris, created_at \
             FROM sbol_oauth_client WHERE client_id = $1",
        )
        .bind(client_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(client_from_row).transpose()
    }

    async fn issue_authorization_code(
        &self,
        code: OAuthAuthorizationCode,
    ) -> Result<(), DomainError> {
        sqlx::query(
            "INSERT INTO sbol_oauth_authorization_code \
             (code_hash, user_id, client_id, redirect_uri, resource, scopes, \
              code_challenge, nonce, expires_at, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(code.code_hash)
        .bind(code.user_id.as_uuid())
        .bind(code.client_id)
        .bind(code.redirect_uri)
        .bind(code.resource)
        .bind(serde_json::to_value(code.scopes)?)
        .bind(code.code_challenge)
        .bind(code.nonce)
        .bind(code.expires_at)
        .bind(code.created_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn consume_authorization_code(
        &self,
        code_hash: &str,
    ) -> Result<Option<OAuthAuthorizationCode>, DomainError> {
        let row = sqlx::query(
            "DELETE FROM sbol_oauth_authorization_code WHERE code_hash = $1 \
             RETURNING code_hash, user_id, client_id, redirect_uri, resource, scopes, \
                       code_challenge, nonce, expires_at, created_at",
        )
        .bind(code_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(code_from_row).transpose()
    }

    async fn issue_access_token(&self, token: OAuthAccessToken) -> Result<(), DomainError> {
        sqlx::query(
            "INSERT INTO sbol_oauth_access_token \
             (token_hash, user_id, client_id, resource, scopes, expires_at, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(token.token_hash)
        .bind(token.user_id.as_uuid())
        .bind(token.client_id)
        .bind(token.resource)
        .bind(serde_json::to_value(token.scopes)?)
        .bind(token.expires_at)
        .bind(token.created_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn resolve_access_token(
        &self,
        token_hash: &str,
    ) -> Result<Option<OAuthAccessToken>, DomainError> {
        let row = sqlx::query(
            "SELECT token_hash, user_id, client_id, resource, scopes, expires_at, created_at \
             FROM sbol_oauth_access_token WHERE token_hash = $1",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(access_from_row).transpose()
    }

    async fn revoke_access_token(&self, token_hash: &str) -> Result<bool, DomainError> {
        let result = sqlx::query("DELETE FROM sbol_oauth_access_token WHERE token_hash = $1")
            .bind(token_hash)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(result.rows_affected() == 1)
    }

    async fn issue_refresh_token(&self, token: OAuthRefreshToken) -> Result<(), DomainError> {
        sqlx::query(
            "INSERT INTO sbol_oauth_refresh_token \
             (token_hash, family_id, user_id, client_id, resource, scopes, expires_at, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(token.token_hash)
        .bind(token.family_id)
        .bind(token.user_id.as_uuid())
        .bind(token.client_id)
        .bind(token.resource)
        .bind(serde_json::to_value(token.scopes)?)
        .bind(token.expires_at)
        .bind(token.created_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn consume_refresh_token(
        &self,
        token_hash: &str,
    ) -> Result<Option<OAuthRefreshToken>, DomainError> {
        let row = sqlx::query(
            "DELETE FROM sbol_oauth_refresh_token WHERE token_hash = $1 \
             RETURNING token_hash, family_id, user_id, client_id, resource, scopes, \
                       expires_at, created_at",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(refresh_from_row).transpose()
    }
}

fn client_from_row(row: PgRow) -> Result<OAuthClient, DomainError> {
    let redirect_uris: serde_json::Value = row.try_get("redirect_uris").map_err(db_err)?;
    Ok(OAuthClient {
        client_id: row.try_get("client_id").map_err(db_err)?,
        client_name: row.try_get("client_name").map_err(db_err)?,
        redirect_uris: serde_json::from_value(redirect_uris)?,
        created_at: row.try_get("created_at").map_err(db_err)?,
    })
}

fn code_from_row(row: PgRow) -> Result<OAuthAuthorizationCode, DomainError> {
    Ok(OAuthAuthorizationCode {
        code_hash: row.try_get("code_hash").map_err(db_err)?,
        user_id: user_id(&row)?,
        client_id: row.try_get("client_id").map_err(db_err)?,
        redirect_uri: row.try_get("redirect_uri").map_err(db_err)?,
        resource: row.try_get("resource").map_err(db_err)?,
        scopes: scopes(&row)?,
        code_challenge: row.try_get("code_challenge").map_err(db_err)?,
        nonce: row.try_get("nonce").map_err(db_err)?,
        expires_at: row.try_get("expires_at").map_err(db_err)?,
        created_at: row.try_get("created_at").map_err(db_err)?,
    })
}

fn access_from_row(row: PgRow) -> Result<OAuthAccessToken, DomainError> {
    Ok(OAuthAccessToken {
        token_hash: row.try_get("token_hash").map_err(db_err)?,
        user_id: user_id(&row)?,
        client_id: row.try_get("client_id").map_err(db_err)?,
        resource: row.try_get("resource").map_err(db_err)?,
        scopes: scopes(&row)?,
        expires_at: row.try_get("expires_at").map_err(db_err)?,
        created_at: row.try_get("created_at").map_err(db_err)?,
    })
}

fn refresh_from_row(row: PgRow) -> Result<OAuthRefreshToken, DomainError> {
    Ok(OAuthRefreshToken {
        token_hash: row.try_get("token_hash").map_err(db_err)?,
        family_id: row.try_get("family_id").map_err(db_err)?,
        user_id: user_id(&row)?,
        client_id: row.try_get("client_id").map_err(db_err)?,
        resource: row.try_get("resource").map_err(db_err)?,
        scopes: scopes(&row)?,
        expires_at: row.try_get("expires_at").map_err(db_err)?,
        created_at: row.try_get("created_at").map_err(db_err)?,
    })
}

fn user_id(row: &PgRow) -> Result<UserId, DomainError> {
    let value: Uuid = row.try_get("user_id").map_err(db_err)?;
    Ok(UserId(value))
}

fn scopes(row: &PgRow) -> Result<Vec<String>, DomainError> {
    let value: serde_json::Value = row.try_get("scopes").map_err(db_err)?;
    Ok(serde_json::from_value(value)?)
}
