//! SBOL Identity OAuth grant lifecycle.
//!
//! This service owns secret generation, one-way hashing, authorization-code
//! PKCE verification, audience binding, scope normalization, expiry, refresh
//! rotation, and revocation. HTTP adapters remain responsible only for OAuth
//! wire shapes and browser consent.

use std::sync::Arc;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{Duration, Utc};
use sbol_db_core::{
    DomainError, OAuthAccessToken, OAuthAuthorizationCode, OAuthClient, OAuthRefreshToken, UserId,
};
use sbol_db_storage::OAuthStore;
use sha3::{Digest, Sha3_256};
use uuid::Uuid;

const AUTHORIZATION_CODE_TTL: Duration = Duration::minutes(5);
const ACCESS_TOKEN_TTL: Duration = Duration::hours(1);
const REFRESH_TOKEN_TTL: Duration = Duration::days(30);

/// The plaintext token response returned once to an OAuth client.
#[derive(Clone)]
pub struct OAuthTokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub scopes: Vec<String>,
    pub resource: String,
    pub user_id: UserId,
    pub client_id: String,
    pub nonce: Option<String>,
}

impl std::fmt::Debug for OAuthTokenPair {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OAuthTokenPair")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("expires_in", &self.expires_in)
            .field("scopes", &self.scopes)
            .field("resource", &self.resource)
            .field("user_id", &self.user_id)
            .field("client_id", &self.client_id)
            .field("nonce", &self.nonce)
            .finish()
    }
}

/// Durable OAuth grant logic over a backend-neutral store.
#[derive(Clone)]
pub struct OAuthService {
    store: Arc<dyn OAuthStore>,
}

impl OAuthService {
    pub fn new(store: Arc<dyn OAuthStore>) -> Self {
        Self { store }
    }

    /// Register a public authorization-code client. Its id is opaque and its
    /// redirect URIs have already been validated by the HTTP adapter.
    pub async fn register_public_client(
        &self,
        client_name: String,
        redirect_uris: Vec<String>,
    ) -> Result<OAuthClient, DomainError> {
        let client = OAuthClient {
            client_id: format!("sbol_client_{}", random_secret()),
            client_name,
            redirect_uris,
            created_at: Utc::now(),
        };
        self.store.register_client(client.clone()).await?;
        Ok(client)
    }

    pub async fn client(&self, client_id: &str) -> Result<Option<OAuthClient>, DomainError> {
        self.store.get_client(client_id).await
    }

    /// Mint a five-minute, single-use authorization code after browser consent.
    #[allow(clippy::too_many_arguments)]
    pub async fn issue_authorization_code(
        &self,
        user_id: UserId,
        client_id: &str,
        redirect_uri: &str,
        resource: &str,
        scopes: Vec<String>,
        code_challenge: &str,
        nonce: Option<String>,
    ) -> Result<String, DomainError> {
        let client = self
            .store
            .get_client(client_id)
            .await?
            .ok_or_else(|| DomainError::InvalidInput("unknown OAuth client".to_owned()))?;
        if !client.redirect_uris.iter().any(|uri| uri == redirect_uri) {
            return Err(DomainError::InvalidInput(
                "redirect_uri is not registered for this client".to_owned(),
            ));
        }
        if code_challenge.is_empty() {
            return Err(DomainError::InvalidInput(
                "an S256 PKCE code challenge is required".to_owned(),
            ));
        }
        let plaintext = format!("sbol_code_{}", random_secret());
        let now = Utc::now();
        self.store
            .issue_authorization_code(OAuthAuthorizationCode {
                code_hash: secret_hash(&plaintext),
                user_id,
                client_id: client_id.to_owned(),
                redirect_uri: redirect_uri.to_owned(),
                resource: resource.to_owned(),
                scopes: normalize_scopes(scopes),
                code_challenge: code_challenge.to_owned(),
                nonce,
                expires_at: now + AUTHORIZATION_CODE_TTL,
                created_at: now,
            })
            .await?;
        Ok(plaintext)
    }

    /// Consume an authorization code, verify the complete request binding and
    /// S256 PKCE verifier, then mint a scoped access/refresh pair.
    pub async fn exchange_authorization_code(
        &self,
        code: &str,
        client_id: &str,
        redirect_uri: &str,
        resource: Option<&str>,
        code_verifier: &str,
    ) -> Result<OAuthTokenPair, DomainError> {
        let grant = self
            .store
            .consume_authorization_code(&secret_hash(code))
            .await?
            .ok_or_else(invalid_grant)?;
        if grant.expires_at <= Utc::now()
            || grant.client_id != client_id
            || grant.redirect_uri != redirect_uri
            || resource.is_some_and(|resource| grant.resource != resource)
            || !constant_time_eq(
                grant.code_challenge.as_bytes(),
                pkce_s256(code_verifier).as_bytes(),
            )
        {
            return Err(invalid_grant());
        }
        self.issue_token_pair(
            grant.user_id,
            grant.client_id,
            grant.resource,
            grant.scopes,
            Uuid::new_v4().to_string(),
            grant.nonce,
        )
        .await
    }

    /// Rotate a refresh token. A failed exchange still consumes the presented
    /// secret, preventing replay probing.
    pub async fn refresh(
        &self,
        refresh_token: &str,
        client_id: &str,
        resource: Option<&str>,
        requested_scopes: Option<Vec<String>>,
    ) -> Result<OAuthTokenPair, DomainError> {
        let grant = self
            .store
            .consume_refresh_token(&secret_hash(refresh_token))
            .await?
            .ok_or_else(invalid_grant)?;
        if grant.expires_at <= Utc::now()
            || grant.client_id != client_id
            || resource.is_some_and(|resource| grant.resource != resource)
        {
            return Err(invalid_grant());
        }
        let scopes = requested_scopes
            .map(normalize_scopes)
            .unwrap_or_else(|| grant.scopes.clone());
        if scopes.iter().any(|scope| !grant.scopes.contains(scope)) {
            return Err(DomainError::InvalidInput(
                "a refresh request cannot expand its original scopes".to_owned(),
            ));
        }
        self.issue_token_pair(
            grant.user_id,
            grant.client_id,
            grant.resource,
            scopes,
            grant.family_id,
            None,
        )
        .await
    }

    /// Resolve a live access token bound to the exact protected resource.
    pub async fn resolve_access_token(
        &self,
        plaintext: &str,
        resource: &str,
    ) -> Result<Option<OAuthAccessToken>, DomainError> {
        let hash = secret_hash(plaintext);
        let Some(token) = self.store.resolve_access_token(&hash).await? else {
            return Ok(None);
        };
        if token.expires_at <= Utc::now() {
            let _ = self.store.revoke_access_token(&hash).await?;
            return Ok(None);
        }
        if token.resource != resource {
            return Ok(None);
        }
        Ok(Some(token))
    }

    pub async fn revoke_access_token(&self, plaintext: &str) -> Result<bool, DomainError> {
        self.store
            .revoke_access_token(&secret_hash(plaintext))
            .await
    }

    /// Revoke either kind of OAuth token without disclosing whether it was
    /// recognized, as required by the revocation endpoint contract.
    pub async fn revoke(&self, plaintext: &str) -> Result<(), DomainError> {
        let hash = secret_hash(plaintext);
        if !self.store.revoke_access_token(&hash).await? {
            let _ = self.store.consume_refresh_token(&hash).await?;
        }
        Ok(())
    }

    async fn issue_token_pair(
        &self,
        user_id: UserId,
        client_id: String,
        resource: String,
        scopes: Vec<String>,
        family_id: String,
        nonce: Option<String>,
    ) -> Result<OAuthTokenPair, DomainError> {
        let access_token = format!("sbol_at_{}", random_secret());
        let refresh_token = format!("sbol_rt_{}", random_secret());
        let now = Utc::now();
        self.store
            .issue_access_token(OAuthAccessToken {
                token_hash: secret_hash(&access_token),
                user_id,
                client_id: client_id.clone(),
                resource: resource.clone(),
                scopes: scopes.clone(),
                expires_at: now + ACCESS_TOKEN_TTL,
                created_at: now,
            })
            .await?;
        self.store
            .issue_refresh_token(OAuthRefreshToken {
                token_hash: secret_hash(&refresh_token),
                family_id,
                user_id,
                client_id: client_id.clone(),
                resource: resource.clone(),
                scopes: scopes.clone(),
                expires_at: now + REFRESH_TOKEN_TTL,
                created_at: now,
            })
            .await?;
        Ok(OAuthTokenPair {
            access_token,
            refresh_token,
            expires_in: ACCESS_TOKEN_TTL.num_seconds(),
            scopes,
            resource,
            user_id,
            client_id,
            nonce,
        })
    }
}

fn normalize_scopes(mut scopes: Vec<String>) -> Vec<String> {
    scopes.retain(|scope| !scope.trim().is_empty());
    for scope in &mut scopes {
        *scope = scope.trim().to_owned();
    }
    scopes.sort();
    scopes.dedup();
    scopes
}

fn random_secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn secret_hash(secret: &str) -> String {
    let mut hasher = Sha3_256::new();
    hasher.update(secret.as_bytes());
    hex::encode(hasher.finalize())
}

fn pkce_s256(verifier: &str) -> String {
    // PKCE S256 is SHA-256, while opaque credential persistence above uses
    // SHA3-256.
    URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

fn invalid_grant() -> DomainError {
    DomainError::Validation("invalid or expired OAuth grant".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::InMemoryOAuthStore;

    #[tokio::test]
    async fn authorization_code_is_pkce_bound_and_single_use() {
        let service = OAuthService::new(Arc::new(InMemoryOAuthStore::new()));
        let client = service
            .register_public_client(
                "Test agent".to_owned(),
                vec!["http://127.0.0.1:43123/callback".to_owned()],
            )
            .await
            .unwrap();
        let verifier = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";
        let code = service
            .issue_authorization_code(
                UserId(Uuid::new_v4()),
                &client.client_id,
                &client.redirect_uris[0],
                "https://sbol.io/mcp",
                vec!["sbol:read".to_owned()],
                &pkce_s256(verifier),
                None,
            )
            .await
            .unwrap();

        let pair = service
            .exchange_authorization_code(
                &code,
                &client.client_id,
                &client.redirect_uris[0],
                Some("https://sbol.io/mcp"),
                verifier,
            )
            .await
            .unwrap();
        assert_eq!(pair.scopes, vec!["sbol:read"]);
        assert!(!format!("{pair:?}").contains(&pair.access_token));
        assert!(!format!("{pair:?}").contains(&pair.refresh_token));
        assert!(service
            .resolve_access_token(&pair.access_token, "https://sbol.io/mcp")
            .await
            .unwrap()
            .is_some());
        assert!(service
            .exchange_authorization_code(
                &code,
                &client.client_id,
                &client.redirect_uris[0],
                Some("https://sbol.io/mcp"),
                verifier,
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn access_tokens_are_audience_bound_and_refresh_cannot_expand_scope() {
        let service = OAuthService::new(Arc::new(InMemoryOAuthStore::new()));
        let client = service
            .register_public_client(
                "Test agent".to_owned(),
                vec!["http://localhost:43123/callback".to_owned()],
            )
            .await
            .unwrap();
        let verifier = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";
        let code = service
            .issue_authorization_code(
                UserId(Uuid::new_v4()),
                &client.client_id,
                &client.redirect_uris[0],
                "https://sbol.io/mcp",
                vec!["sbol:read".to_owned()],
                &pkce_s256(verifier),
                None,
            )
            .await
            .unwrap();
        let pair = service
            .exchange_authorization_code(
                &code,
                &client.client_id,
                &client.redirect_uris[0],
                Some("https://sbol.io/mcp"),
                verifier,
            )
            .await
            .unwrap();

        assert!(service
            .resolve_access_token(&pair.access_token, "https://other.example/mcp")
            .await
            .unwrap()
            .is_none());
        assert!(service
            .resolve_access_token(&pair.access_token, "https://sbol.io/mcp")
            .await
            .unwrap()
            .is_some());
        assert!(service
            .refresh(
                &pair.refresh_token,
                &client.client_id,
                Some("https://sbol.io/mcp"),
                Some(vec!["sbol:write".to_owned()]),
            )
            .await
            .is_err());
    }
}
