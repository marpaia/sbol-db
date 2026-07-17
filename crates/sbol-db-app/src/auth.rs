//! Password and API-token authentication.
//!
//! [`AuthService`] owns the credential logic every adapter shares. It registers
//! accounts with an argon2id password hash, authenticates a caller by username
//! or email, and mints and resolves API tokens.
//!
//! Authentication accepts two stored-hash formats so an instance migrated from
//! classic SynBioHub keeps working: a modern argon2 PHC string, and the legacy
//! `sha1(salt + sha1(password))` digest SynBioHub wrote (`lib/db.js`). A
//! successful login against a legacy digest transparently re-hashes the
//! password to argon2 and persists it through [`UserStore::set_password_hash`],
//! so stored credentials upgrade as users sign in.

use std::sync::Arc;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use sbol_db_core::{DomainError, NewUser, User, UserId};
use sbol_db_storage::{TokenStore, UserStore};
use sha1::Sha1;
use sha3::{Digest, Sha3_256};
use uuid::Uuid;

/// The fields needed to register an account. The plaintext `password` is
/// argon2-hashed and the `graph_uri` is derived from `username`; the store
/// assigns the [`UserId`](sbol_db_core::UserId).
#[derive(Clone, Debug)]
pub struct Registration {
    pub username: String,
    pub name: String,
    pub email: String,
    pub affiliation: Option<String>,
    pub password: String,
    pub is_admin: bool,
    pub is_curator: bool,
    pub is_member: bool,
}

/// The result of initiating a password reset: the account the reset is for and
/// the single-use link now stored against it. The caller (the reset route)
/// turns this into the `send_email` job that delivers the link.
#[derive(Clone, Debug)]
pub struct PasswordReset {
    pub username: String,
    pub email: String,
    pub reset_link: String,
}

/// Password and API-token authentication over the identity stores.
#[derive(Clone)]
pub struct AuthService {
    users: Arc<dyn UserStore>,
    tokens: Arc<dyn TokenStore>,
}

impl AuthService {
    /// Build the service over the identity stores.
    pub fn new(users: Arc<dyn UserStore>, tokens: Arc<dyn TokenStore>) -> Self {
        Self { users, tokens }
    }

    /// The named graph an account owns, the identity-to-RDF bridge ACL scoping
    /// keys on. Matches SynBioHub's `databasePrefix + user/<username>`.
    pub fn graph_uri(username: &str) -> String {
        format!("http://synbiohub.org/user/{username}")
    }

    /// Create an account, argon2-hashing the plaintext password and stamping
    /// the owned graph URI, and return the stored account.
    pub async fn register(&self, registration: Registration) -> Result<User, DomainError> {
        // Reject a duplicate username or email up front so a re-registration is
        // a client error, not the database's unique-constraint failure surfacing
        // as a 500.
        for identifier in [&registration.username, &registration.email] {
            if self
                .users
                .find_by_email_or_username(identifier)
                .await?
                .is_some()
            {
                return Err(DomainError::InvalidInput(
                    "username or email already registered".into(),
                ));
            }
        }
        let password_hash = hash_password(&registration.password)?;
        let graph_uri = Self::graph_uri(&registration.username);
        let new_user = NewUser {
            username: registration.username,
            name: registration.name,
            email: registration.email,
            affiliation: registration.affiliation,
            password_hash,
            graph_uri,
            is_admin: registration.is_admin,
            is_curator: registration.is_curator,
            is_member: registration.is_member,
        };
        self.users.create_user(new_user).await
    }

    /// Resolve `identifier` (matching either email or username) and verify
    /// `password` against the stored hash, accepting both argon2 and the legacy
    /// `sha1(salt + sha1(password))` format. On a successful legacy verify the
    /// password is re-hashed to argon2 and persisted (rehash-on-login). Returns
    /// the authenticated account.
    pub async fn authenticate(
        &self,
        identifier: &str,
        password: &str,
        salt: &str,
    ) -> Result<User, DomainError> {
        let user = self
            .users
            .find_by_email_or_username(identifier)
            .await?
            .ok_or_else(|| DomainError::Validation("invalid credentials".into()))?;

        let is_legacy = !is_argon2(&user.password_hash);
        if !verify_password(&user.password_hash, password, salt)? {
            return Err(DomainError::Validation("invalid credentials".into()));
        }

        if is_legacy {
            let rehashed = hash_password(password)?;
            self.users.set_password_hash(user.id, &rehashed).await?;
        }

        Ok(user)
    }

    /// Mint an API token for `user_id`: generate a UUID v4, persist only its
    /// sha3-256 hash, and return the plaintext token (the sole copy the caller
    /// ever sees).
    pub async fn issue_token(&self, user_id: UserId) -> Result<String, DomainError> {
        let token = Uuid::new_v4().to_string();
        self.tokens.issue(&token_hash(&token), user_id).await?;
        Ok(token)
    }

    /// Resolve a plaintext token to the account it authenticates, or `None`
    /// when no live token carries its hash.
    pub async fn resolve_token(&self, token: &str) -> Result<Option<UserId>, DomainError> {
        self.tokens.resolve(&token_hash(token)).await
    }

    /// Revoke a plaintext token, returning whether a live token was removed.
    pub async fn revoke_token(&self, token: &str) -> Result<bool, DomainError> {
        self.tokens.revoke(&token_hash(token)).await
    }

    /// Begin a password reset for the account matching `identifier` (email or
    /// username): mint a single-use reset link, store it against the account,
    /// and return the details the caller enqueues a delivery email with.
    /// Returns `None` when no account matches, so the caller can respond
    /// identically whether or not the address is registered.
    pub async fn reset_password(
        &self,
        identifier: &str,
    ) -> Result<Option<PasswordReset>, DomainError> {
        let Some(user) = self.users.find_by_email_or_username(identifier).await? else {
            return Ok(None);
        };
        let reset_link = Uuid::new_v4().to_string();
        self.users
            .set_reset_link(user.id, Some(&reset_link))
            .await?;
        Ok(Some(PasswordReset {
            username: user.username,
            email: user.email,
            reset_link,
        }))
    }

    /// Complete a password reset: atomically claim the account carrying
    /// `reset_link` (clearing the link), argon2-hash `new_password`, persist it,
    /// and return the account. Errors when the link matches no account.
    pub async fn set_new_password(
        &self,
        reset_link: &str,
        new_password: &str,
    ) -> Result<User, DomainError> {
        let user = self
            .users
            .consume_reset_link(reset_link)
            .await?
            .ok_or_else(|| DomainError::Validation("invalid or expired reset link".into()))?;
        let password_hash = hash_password(new_password)?;
        self.users
            .set_password_hash(user.id, &password_hash)
            .await?;
        Ok(user)
    }
}

/// Whether a stored hash is an argon2 PHC string (as opposed to a legacy
/// SynBioHub digest).
fn is_argon2(stored_hash: &str) -> bool {
    stored_hash.starts_with("$argon2")
}

/// Argon2id PHC string for `password` with a fresh random salt.
fn hash_password(password: &str) -> Result<String, DomainError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| DomainError::Database(format!("password hashing failed: {e}")))
}

/// Verify `password` against `stored_hash`, dispatching on its format: an
/// argon2 PHC string is verified with argon2; anything else is treated as the
/// legacy `sha1(password_salt + sha1(password))` digest.
fn verify_password(stored_hash: &str, password: &str, salt: &str) -> Result<bool, DomainError> {
    if is_argon2(stored_hash) {
        let parsed = PasswordHash::new(stored_hash)
            .map_err(|e| DomainError::Database(format!("stored password hash is invalid: {e}")))?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    } else {
        Ok(legacy_hash(salt, password).eq_ignore_ascii_case(stored_hash.trim()))
    }
}

/// The classic SynBioHub password digest `sha1(salt + sha1(password))`, where
/// each `sha1` yields lowercase hex (`lib/db.js`).
fn legacy_hash(salt: &str, password: &str) -> String {
    let inner = hex::encode(Sha1::digest(password.as_bytes()));
    hex::encode(Sha1::digest(format!("{salt}{inner}").as_bytes()))
}

/// The stored form of an API token: sha3-256 of the plaintext, hex-encoded.
fn token_hash(token: &str) -> String {
    hex::encode(Sha3_256::digest(token.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{InMemoryTokenStore, InMemoryUserStore};

    const SALT: &str = "test-password-salt";

    fn service() -> (AuthService, Arc<InMemoryUserStore>) {
        let users = Arc::new(InMemoryUserStore::new());
        let tokens = Arc::new(InMemoryTokenStore::new());
        let auth = AuthService::new(users.clone(), tokens);
        (auth, users)
    }

    fn registration(password: &str) -> Registration {
        Registration {
            username: "alice".into(),
            name: "Alice".into(),
            email: "alice@example.org".into(),
            affiliation: None,
            password: password.into(),
            is_admin: false,
            is_curator: false,
            is_member: true,
        }
    }

    #[tokio::test]
    async fn duplicate_registration_is_invalid_input() {
        let (auth, _) = service();
        auth.register(registration("s3cret")).await.unwrap();

        // Same username and email: rejected as a client error, not a database
        // unique-constraint failure surfacing as an internal error.
        let err = auth.register(registration("other")).await.unwrap_err();
        assert!(
            matches!(err, DomainError::InvalidInput(_)),
            "re-registration must be InvalidInput, got {err:?}"
        );
    }

    #[tokio::test]
    async fn argon2_login_roundtrip() {
        let (auth, _) = service();
        let user = auth.register(registration("s3cret")).await.unwrap();
        assert!(is_argon2(&user.password_hash));

        let by_username = auth.authenticate("alice", "s3cret", SALT).await.unwrap();
        assert_eq!(by_username.id, user.id);
        let by_email = auth
            .authenticate("alice@example.org", "s3cret", SALT)
            .await
            .unwrap();
        assert_eq!(by_email.id, user.id);
    }

    #[tokio::test]
    async fn wrong_password_rejected() {
        let (auth, _) = service();
        auth.register(registration("s3cret")).await.unwrap();
        let err = auth.authenticate("alice", "wrong", SALT).await.unwrap_err();
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[tokio::test]
    async fn legacy_sha1_login_verifies() {
        let (auth, users) = service();
        let seeded = NewUser {
            username: "bob".into(),
            name: "Bob".into(),
            email: "bob@example.org".into(),
            affiliation: None,
            password_hash: legacy_hash(SALT, "hunter2"),
            graph_uri: AuthService::graph_uri("bob"),
            is_admin: false,
            is_curator: false,
            is_member: true,
        };
        users.create_user(seeded).await.unwrap();

        let user = auth.authenticate("bob", "hunter2", SALT).await.unwrap();
        assert_eq!(user.username, "bob");
        assert!(auth.authenticate("bob", "nope", SALT).await.is_err());
    }

    #[tokio::test]
    async fn legacy_verify_triggers_rehash_to_argon2() {
        let (auth, users) = service();
        let legacy = legacy_hash(SALT, "hunter2");
        let seeded = NewUser {
            username: "carol".into(),
            name: "Carol".into(),
            email: "carol@example.org".into(),
            affiliation: None,
            password_hash: legacy.clone(),
            graph_uri: AuthService::graph_uri("carol"),
            is_admin: false,
            is_curator: false,
            is_member: true,
        };
        let created = users.create_user(seeded).await.unwrap();
        assert!(!is_argon2(&created.password_hash));

        auth.authenticate("carol", "hunter2", SALT).await.unwrap();

        let stored = users.get_by_id(created.id).await.unwrap().unwrap();
        assert!(is_argon2(&stored.password_hash));
        assert_ne!(stored.password_hash, legacy);

        // The upgraded hash still authenticates the same password.
        auth.authenticate("carol", "hunter2", SALT).await.unwrap();
    }

    #[tokio::test]
    async fn token_issue_then_resolve_roundtrips() {
        let (auth, _) = service();
        let user = auth.register(registration("s3cret")).await.unwrap();
        let token = auth.issue_token(user.id).await.unwrap();
        assert_eq!(auth.resolve_token(&token).await.unwrap(), Some(user.id));
        assert_eq!(auth.resolve_token("not-a-token").await.unwrap(), None);
    }

    #[tokio::test]
    async fn revoke_token_makes_it_unresolvable() {
        let (auth, _) = service();
        let user = auth.register(registration("s3cret")).await.unwrap();
        let token = auth.issue_token(user.id).await.unwrap();
        assert!(auth.revoke_token(&token).await.unwrap());
        assert_eq!(auth.resolve_token(&token).await.unwrap(), None);
        // A second revoke is a no-op.
        assert!(!auth.revoke_token(&token).await.unwrap());
    }

    #[tokio::test]
    async fn reset_then_set_new_password_authenticates() {
        let (auth, _) = service();
        auth.register(registration("s3cret")).await.unwrap();

        let reset = auth
            .reset_password("alice@example.org")
            .await
            .unwrap()
            .expect("account matched");
        assert_eq!(reset.username, "alice");

        // The old password no longer authenticates once a new one is set.
        let user = auth
            .set_new_password(&reset.reset_link, "n3w-pass")
            .await
            .unwrap();
        assert_eq!(user.username, "alice");
        assert!(auth.authenticate("alice", "n3w-pass", SALT).await.is_ok());
        assert!(auth.authenticate("alice", "s3cret", SALT).await.is_err());

        // The link is single-use.
        assert!(auth
            .set_new_password(&reset.reset_link, "again")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn reset_password_unknown_identifier_is_none() {
        let (auth, _) = service();
        assert!(auth
            .reset_password("nobody@example.org")
            .await
            .unwrap()
            .is_none());
    }
}
