//! The account store over RocksDB.
//!
//! The `users` column family holds each account as JSON keyed by its
//! [`UserId`] bytes; two secondary families map `username` and `email` to that
//! id so [`UserStore::find_by_email_or_username`] is a point lookup. Mutations
//! take a process-wide lock so the account row and its indexes stay consistent;
//! reads are lock-free. Password hashing lives in the application layer.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rocksdb::WriteBatch;
use sbol_db_core::{DomainError, NewUser, User, UserId};
use sbol_db_storage::UserStore;
use uuid::Uuid;

use crate::db::Db;

const CF_USERS: &str = "users";
const CF_BY_USERNAME: &str = "users_by_username";
const CF_BY_EMAIL: &str = "users_by_email";

#[derive(Clone)]
pub struct RocksdbUserStore {
    db: Db,
    writes: Arc<Mutex<()>>,
}

impl RocksdbUserStore {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            writes: Arc::new(Mutex::new(())),
        }
    }
}

#[async_trait]
impl UserStore for RocksdbUserStore {
    async fn create_user(&self, new_user: NewUser) -> Result<User, DomainError> {
        let db = self.db.clone();
        let writes = self.writes.clone();
        blocking(move || {
            let _guard = writes.lock().unwrap();
            if db.exists_cf(CF_BY_USERNAME, new_user.username.as_bytes())? {
                return Err(DomainError::Database(format!(
                    "username `{}` already exists",
                    new_user.username
                )));
            }
            if db.exists_cf(CF_BY_EMAIL, new_user.email.as_bytes())? {
                return Err(DomainError::Database(format!(
                    "email `{}` already exists",
                    new_user.email
                )));
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
            put_user(&db, &user)?;
            Ok(user)
        })
        .await
    }

    async fn find_by_email_or_username(
        &self,
        identifier: &str,
    ) -> Result<Option<User>, DomainError> {
        let db = self.db.clone();
        let identifier = identifier.to_owned();
        blocking(move || {
            let id = match db.get_cf(CF_BY_EMAIL, identifier.as_bytes())? {
                Some(bytes) => Some(user_id_from_bytes(&bytes)?),
                None => match db.get_cf(CF_BY_USERNAME, identifier.as_bytes())? {
                    Some(bytes) => Some(user_id_from_bytes(&bytes)?),
                    None => None,
                },
            };
            match id {
                Some(id) => get_user(&db, id),
                None => Ok(None),
            }
        })
        .await
    }

    async fn get_by_id(&self, id: UserId) -> Result<Option<User>, DomainError> {
        let db = self.db.clone();
        blocking(move || get_user(&db, id)).await
    }

    async fn update_user(&self, user: &User) -> Result<User, DomainError> {
        let db = self.db.clone();
        let writes = self.writes.clone();
        let id = user.id;
        let name = user.name.clone();
        let affiliation = user.affiliation.clone();
        let is_admin = user.is_admin;
        let is_curator = user.is_curator;
        let is_member = user.is_member;
        blocking(move || {
            let _guard = writes.lock().unwrap();
            let mut stored =
                get_user(&db, id)?.ok_or_else(|| DomainError::NotFound(format!("user {id}")))?;
            stored.name = name;
            stored.affiliation = affiliation;
            stored.is_admin = is_admin;
            stored.is_curator = is_curator;
            stored.is_member = is_member;
            put_user(&db, &stored)?;
            Ok(stored)
        })
        .await
    }

    async fn set_password_hash(&self, id: UserId, password_hash: &str) -> Result<(), DomainError> {
        let db = self.db.clone();
        let writes = self.writes.clone();
        let password_hash = password_hash.to_owned();
        blocking(move || {
            let _guard = writes.lock().unwrap();
            let mut stored =
                get_user(&db, id)?.ok_or_else(|| DomainError::NotFound(format!("user {id}")))?;
            stored.password_hash = password_hash;
            put_user(&db, &stored)
        })
        .await
    }

    async fn set_reset_link(&self, id: UserId, link: Option<&str>) -> Result<(), DomainError> {
        let db = self.db.clone();
        let writes = self.writes.clone();
        let link = link.map(|s| s.to_owned());
        blocking(move || {
            let _guard = writes.lock().unwrap();
            let mut stored =
                get_user(&db, id)?.ok_or_else(|| DomainError::NotFound(format!("user {id}")))?;
            stored.reset_password_link = link;
            put_user(&db, &stored)
        })
        .await
    }

    async fn consume_reset_link(&self, link: &str) -> Result<Option<User>, DomainError> {
        let db = self.db.clone();
        let writes = self.writes.clone();
        let link = link.to_owned();
        blocking(move || {
            let _guard = writes.lock().unwrap();
            let mut found: Option<User> = None;
            db.for_each(CF_USERS, |_, blob| {
                let user: User = decode(blob)?;
                if user.reset_password_link.as_deref() == Some(link.as_str()) {
                    found = Some(user);
                    return Ok(false);
                }
                Ok(true)
            })?;
            match found {
                Some(mut user) => {
                    user.reset_password_link = None;
                    put_user(&db, &user)?;
                    Ok(Some(user))
                }
                None => Ok(None),
            }
        })
        .await
    }

    async fn delete_user(&self, id: UserId) -> Result<bool, DomainError> {
        let db = self.db.clone();
        let writes = self.writes.clone();
        blocking(move || {
            let _guard = writes.lock().unwrap();
            let Some(user) = get_user(&db, id)? else {
                return Ok(false);
            };
            let mut batch = WriteBatch::default();
            batch.delete_cf(&db.cf(CF_USERS), id.as_uuid().as_bytes());
            batch.delete_cf(&db.cf(CF_BY_USERNAME), user.username.as_bytes());
            batch.delete_cf(&db.cf(CF_BY_EMAIL), user.email.as_bytes());
            db.write(batch)?;
            Ok(true)
        })
        .await
    }
}

/// Write an account plus its username/email lookup entries in one batch.
fn put_user(db: &Db, user: &User) -> Result<(), DomainError> {
    let id_bytes = user.id.as_uuid().into_bytes();
    let mut batch = WriteBatch::default();
    batch.put_cf(&db.cf(CF_USERS), id_bytes, encode(user)?);
    batch.put_cf(&db.cf(CF_BY_USERNAME), user.username.as_bytes(), id_bytes);
    batch.put_cf(&db.cf(CF_BY_EMAIL), user.email.as_bytes(), id_bytes);
    db.write(batch)
}

fn get_user(db: &Db, id: UserId) -> Result<Option<User>, DomainError> {
    match db.get_cf(CF_USERS, id.as_uuid().as_bytes())? {
        Some(blob) => Ok(Some(decode(&blob)?)),
        None => Ok(None),
    }
}

fn user_id_from_bytes(bytes: &[u8]) -> Result<UserId, DomainError> {
    let uuid = Uuid::from_slice(bytes).map_err(|_| DomainError::Database("bad user id".into()))?;
    Ok(UserId(uuid))
}

async fn blocking<T, F>(f: F) -> Result<T, DomainError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, DomainError> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| DomainError::Database(format!("rocksdb task panicked: {e}")))?
}

fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, DomainError> {
    serde_json::to_vec(value).map_err(|e| DomainError::Serialization(e.to_string()))
}

fn decode<T: for<'de> serde::Deserialize<'de>>(blob: &[u8]) -> Result<T, DomainError> {
    serde_json::from_slice(blob).map_err(|e| DomainError::Serialization(e.to_string()))
}
