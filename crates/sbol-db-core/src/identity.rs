//! Identity domain types: the account a caller authenticates as and the API
//! token that carries that identity on a request.
//!
//! These mirror SynBioHub's user model so a migrated instance keeps the same
//! account fields and derived user graph URI. No hashing or I/O lives here; the
//! storage traits persist these values and the application facade computes the
//! password and token hashes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::UserId;

/// A registered account. `password_hash` holds either an argon2 PHC string or a
/// migrated legacy `sha1(salt + sha1(password))` digest; the application layer
/// distinguishes them. `graph_uri` is the account's owned named graph
/// (`http://synbiohub.org/user/<username>`) that ACL-scoped reads key on.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub username: String,
    pub name: String,
    pub email: String,
    pub affiliation: Option<String>,
    pub password_hash: String,
    pub graph_uri: String,
    pub is_admin: bool,
    pub is_curator: bool,
    pub is_member: bool,
    /// A single-use password-reset token, set while a reset is outstanding and
    /// cleared once consumed.
    pub reset_password_link: Option<String>,
    /// When the account was created, in UTC. The store sets it once on create.
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    /// When the account was last modified, in UTC. The store sets it on create
    /// and bumps it on a profile or password change.
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
}

/// The fields needed to create a [`User`]. The store assigns the [`UserId`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NewUser {
    pub username: String,
    pub name: String,
    pub email: String,
    pub affiliation: Option<String>,
    pub password_hash: String,
    pub graph_uri: String,
    pub is_admin: bool,
    pub is_curator: bool,
    pub is_member: bool,
}

/// A persisted API token. Only the hash of the plaintext token is stored, so a
/// leaked row cannot be replayed; the plaintext exists only in the response
/// that mints it and in the client that presents it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiToken {
    pub token_hash: String,
    pub user_id: UserId,
    pub created_at: DateTime<Utc>,
}
