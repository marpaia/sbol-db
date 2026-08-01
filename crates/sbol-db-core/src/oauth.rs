//! OAuth authorization domain records shared by the identity service and
//! durable storage backends.
//!
//! Secrets are represented only by hashes. Plain authorization codes, access
//! tokens, and refresh tokens exist solely at issuance and at the client.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::UserId;

/// A public OAuth client registration.
///
/// SBOL Identity initially supports public authorization-code clients with
/// PKCE. Confidential client credentials can be added without changing the
/// authorization grant records below.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OAuthClient {
    pub client_id: String,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// A short-lived, single-use authorization-code grant.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OAuthAuthorizationCode {
    pub code_hash: String,
    pub user_id: UserId,
    pub client_id: String,
    pub redirect_uri: String,
    pub resource: String,
    pub scopes: Vec<String>,
    pub code_challenge: String,
    /// OpenID Connect nonce echoed into the ID token when the `openid` scope
    /// was authorized.
    pub nonce: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// A scoped, audience-bound OAuth access token.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OAuthAccessToken {
    pub token_hash: String,
    pub user_id: UserId,
    pub client_id: String,
    pub resource: String,
    pub scopes: Vec<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// A rotating refresh token. `family_id` ties a sequence of rotations together
/// so a later implementation can revoke an entire family on replay.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OAuthRefreshToken {
    pub token_hash: String,
    pub family_id: String,
    pub user_id: UserId,
    pub client_id: String,
    pub resource: String,
    pub scopes: Vec<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}
