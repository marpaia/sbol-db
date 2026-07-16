//! In-memory identity stores.
//!
//! [`InMemoryUserStore`] and [`InMemoryTokenStore`] implement the identity
//! storage traits against process-local maps. They back two callers: the
//! [`AppServices::new`](crate::AppServices::new) convenience constructor, which
//! provisions a non-persistent identity layer for callers assembling the facade
//! from individual handles, and the facade's own tests and adapter integration
//! tests, which drive [`AuthService`](crate::AuthService) without a database.
//! A real deployment gets its persistent user and token stores through
//! [`AppServices::from_backend`](crate::AppServices::from_backend).

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use sbol_db_core::{DomainError, NewUser, User, UserId};
use sbol_db_storage::{TokenStore, UserStore};

/// A process-local [`UserStore`] keyed by [`UserId`], enforcing the same unique
/// `username` and `email` constraints as the persistent backends.
#[derive(Default)]
pub struct InMemoryUserStore {
    users: Mutex<HashMap<UserId, User>>,
}

impl InMemoryUserStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl UserStore for InMemoryUserStore {
    async fn create_user(&self, new_user: NewUser) -> Result<User, DomainError> {
        let mut users = self.users.lock().unwrap();
        if users
            .values()
            .any(|u| u.username == new_user.username || u.email == new_user.email)
        {
            return Err(DomainError::InvalidInput(
                "username or email already registered".into(),
            ));
        }
        let user = User {
            id: UserId::new(),
            username: new_user.username,
            name: new_user.name,
            email: new_user.email,
            affiliation: new_user.affiliation,
            password_hash: new_user.password_hash,
            graph_uri: new_user.graph_uri,
            is_admin: new_user.is_admin,
            is_curator: new_user.is_curator,
            is_member: new_user.is_member,
            reset_password_link: None,
        };
        users.insert(user.id, user.clone());
        Ok(user)
    }

    async fn find_by_email_or_username(
        &self,
        identifier: &str,
    ) -> Result<Option<User>, DomainError> {
        let users = self.users.lock().unwrap();
        Ok(users
            .values()
            .find(|u| u.email == identifier || u.username == identifier)
            .cloned())
    }

    async fn get_by_id(&self, id: UserId) -> Result<Option<User>, DomainError> {
        Ok(self.users.lock().unwrap().get(&id).cloned())
    }

    async fn update_user(&self, user: &User) -> Result<User, DomainError> {
        let mut users = self.users.lock().unwrap();
        let Some(existing) = users.get_mut(&user.id) else {
            return Err(DomainError::NotFound(format!("user {}", user.id)));
        };
        existing.name = user.name.clone();
        existing.affiliation = user.affiliation.clone();
        existing.is_admin = user.is_admin;
        existing.is_curator = user.is_curator;
        existing.is_member = user.is_member;
        Ok(existing.clone())
    }

    async fn set_password_hash(&self, id: UserId, password_hash: &str) -> Result<(), DomainError> {
        let mut users = self.users.lock().unwrap();
        let Some(user) = users.get_mut(&id) else {
            return Err(DomainError::NotFound(format!("user {id}")));
        };
        user.password_hash = password_hash.to_owned();
        Ok(())
    }

    async fn set_reset_link(&self, id: UserId, link: Option<&str>) -> Result<(), DomainError> {
        let mut users = self.users.lock().unwrap();
        let Some(user) = users.get_mut(&id) else {
            return Err(DomainError::NotFound(format!("user {id}")));
        };
        user.reset_password_link = link.map(str::to_owned);
        Ok(())
    }

    async fn consume_reset_link(&self, link: &str) -> Result<Option<User>, DomainError> {
        let mut users = self.users.lock().unwrap();
        let id = users
            .values()
            .find(|u| u.reset_password_link.as_deref() == Some(link))
            .map(|u| u.id);
        let Some(id) = id else {
            return Ok(None);
        };
        let user = users.get_mut(&id).unwrap();
        user.reset_password_link = None;
        Ok(Some(user.clone()))
    }
}

/// A process-local [`TokenStore`] mapping a token hash to the account it
/// authenticates.
#[derive(Default)]
pub struct InMemoryTokenStore {
    tokens: Mutex<HashMap<String, UserId>>,
}

impl InMemoryTokenStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl TokenStore for InMemoryTokenStore {
    async fn issue(&self, token_hash: &str, user_id: UserId) -> Result<(), DomainError> {
        self.tokens
            .lock()
            .unwrap()
            .insert(token_hash.to_owned(), user_id);
        Ok(())
    }

    async fn resolve(&self, token_hash: &str) -> Result<Option<UserId>, DomainError> {
        Ok(self.tokens.lock().unwrap().get(token_hash).copied())
    }

    async fn revoke(&self, token_hash: &str) -> Result<bool, DomainError> {
        Ok(self.tokens.lock().unwrap().remove(token_hash).is_some())
    }
}
