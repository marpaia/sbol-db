//! The account store over RocksDB.
//!
//! The `users` column family holds each account as JSON keyed by its
//! [`UserId`] bytes; two secondary families map `username` and `email` to that
//! id so [`UserStore::find_by_email_or_username`] is a point lookup. Mutations
//! take a process-wide lock so the account row and its indexes stay consistent;
//! reads are lock-free. Password hashing lives in the application layer.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;
use rocksdb::WriteBatch;
use sbol_db_core::{DomainError, NewUser, User, UserId};
use sbol_db_storage::UserStore;
use uuid::Uuid;

use crate::db::{Db, SEP};

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

    /// Import already-materialized accounts without regenerating ids or
    /// timestamps. Backend conversion uses this to preserve the production
    /// identity rows exactly, including classic duplicate-email accounts.
    pub async fn import_exact(&self, users: Vec<User>) -> Result<(), DomainError> {
        let db = self.db.clone();
        let writes = self.writes.clone();
        blocking(move || {
            let _guard = writes.lock().unwrap();
            let mut batch = WriteBatch::default();
            for user in &users {
                if let Some(existing) = db.get_cf(CF_BY_USERNAME, user.username.as_bytes())? {
                    if existing.as_slice() != user.id.as_uuid().as_bytes() {
                        return Err(DomainError::Database(format!(
                            "username `{}` already belongs to another account",
                            user.username
                        )));
                    }
                }
                stage_user(&db, &mut batch, user)?;
            }
            db.write(batch)
        })
        .await
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
            let now = Utc::now();
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
                created_at: now,
                updated_at: now,
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
            if let Some(bytes) = db.get_cf(CF_BY_USERNAME, identifier.as_bytes())? {
                return get_user(&db, user_id_from_bytes(&bytes)?);
            }
            let prefix = email_prefix(&identifier);
            let mut ids = Vec::new();
            db.for_each_prefix(CF_BY_EMAIL, &prefix, |key, _| {
                ids.push(user_id_from_email_key(&prefix, key)?);
                Ok(ids.len() < 2)
            })?;
            // Version-1 databases used one direct `email -> id` row. Include
            // that row during the transition; once the same user is rewritten
            // into the composite index, dedup keeps it from looking ambiguous.
            if ids.len() < 2 {
                if let Some(legacy) = db.get_cf(CF_BY_EMAIL, identifier.as_bytes())? {
                    let id = user_id_from_bytes(&legacy)?;
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
            match ids.len() {
                0 => Ok(None),
                1 => get_user(&db, ids[0]),
                _ => Err(DomainError::Validation(
                    "multiple accounts use this email; log in with your username".to_owned(),
                )),
            }
        })
        .await
    }

    async fn get_by_id(&self, id: UserId) -> Result<Option<User>, DomainError> {
        let db = self.db.clone();
        blocking(move || get_user(&db, id)).await
    }

    async fn list_users(&self) -> Result<Vec<User>, DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut users = Vec::new();
            db.for_each(CF_USERS, |_, value| {
                users.push(decode::<User>(value)?);
                Ok(true)
            })?;
            users.sort_by(|left, right| left.username.cmp(&right.username));
            Ok(users)
        })
        .await
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
            stored.updated_at = Utc::now();
            put_user(&db, &stored)?;
            Ok(stored)
        })
        .await
    }

    async fn set_sole_admin(&self, id: UserId) -> Result<(), DomainError> {
        let db = self.db.clone();
        let writes = self.writes.clone();
        blocking(move || {
            let _guard = writes.lock().unwrap();
            if get_user(&db, id)?.is_none() {
                return Err(DomainError::NotFound(format!("user {id}")));
            }

            let now = Utc::now();
            let mut users = Vec::new();
            db.for_each(CF_USERS, |_, value| {
                users.push(decode::<User>(value)?);
                Ok(true)
            })?;

            let mut batch = WriteBatch::default();
            for mut user in users {
                let should_be_admin = user.id == id;
                if user.is_admin != should_be_admin {
                    user.is_admin = should_be_admin;
                    user.updated_at = now;
                    stage_user(&db, &mut batch, &user)?;
                }
            }
            db.write(batch)
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
            stored.updated_at = Utc::now();
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
            batch.delete_cf(&db.cf(CF_BY_EMAIL), email_key(&user.email, id));
            if db
                .get_cf(CF_BY_EMAIL, user.email.as_bytes())?
                .is_some_and(|legacy| legacy.as_slice() == id.as_uuid().as_bytes())
            {
                batch.delete_cf(&db.cf(CF_BY_EMAIL), user.email.as_bytes());
            }
            db.write(batch)?;
            Ok(true)
        })
        .await
    }

    async fn any_admin(&self) -> Result<bool, DomainError> {
        let db = self.db.clone();
        blocking(move || {
            let mut found = false;
            db.for_each(CF_USERS, |_, value| {
                if decode::<User>(value)?.is_admin {
                    found = true;
                    return Ok(false);
                }
                Ok(true)
            })?;
            Ok(found)
        })
        .await
    }
}

/// Write an account plus its username/email lookup entries in one batch.
fn put_user(db: &Db, user: &User) -> Result<(), DomainError> {
    let mut batch = WriteBatch::default();
    stage_user(db, &mut batch, user)?;
    db.write(batch)
}

fn stage_user(db: &Db, batch: &mut WriteBatch, user: &User) -> Result<(), DomainError> {
    let id_bytes = user.id.as_uuid().into_bytes();
    batch.put_cf(&db.cf(CF_USERS), id_bytes, encode(user)?);
    batch.put_cf(&db.cf(CF_BY_USERNAME), user.username.as_bytes(), id_bytes);
    batch.put_cf(&db.cf(CF_BY_EMAIL), email_key(&user.email, user.id), []);
    Ok(())
}

fn email_prefix(email: &str) -> Vec<u8> {
    let mut key = email.as_bytes().to_vec();
    key.push(SEP);
    key
}

fn email_key(email: &str, id: UserId) -> Vec<u8> {
    let mut key = email_prefix(email);
    key.extend_from_slice(id.as_uuid().as_bytes());
    key
}

fn user_id_from_email_key(prefix: &[u8], key: &[u8]) -> Result<UserId, DomainError> {
    key.strip_prefix(prefix)
        .ok_or_else(|| DomainError::Database("email index key has the wrong prefix".into()))
        .and_then(user_id_from_bytes)
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
