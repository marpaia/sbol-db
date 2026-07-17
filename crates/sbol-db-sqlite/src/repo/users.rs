//! The account store over SQLite.
//!
//! Mirrors the Postgres semantics: rows map to [`User`], `username` and `email`
//! are unique, and the store persists an already-computed `password_hash`.
//! UUIDs are TEXT and booleans are INTEGER (0/1).

use async_trait::async_trait;
use chrono::Utc;
use sbol_db_core::{DomainError, NewUser, User, UserId};
use sbol_db_storage::UserStore;
use sqlx::Row;

use crate::pool::db_err;
use crate::SqlitePool;

const USER_COLS: &str = "id, username, name, email, affiliation, password_hash, \
    graph_uri, is_admin, is_curator, is_member, reset_password_link, created_at, updated_at";

#[derive(Clone)]
pub struct SqliteUserStore {
    pool: SqlitePool,
}

impl SqliteUserStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl UserStore for SqliteUserStore {
    async fn create_user(&self, new_user: NewUser) -> Result<User, DomainError> {
        let id = UserId::new();
        let now = Utc::now();
        let row = sqlx::query(&format!(
            "INSERT INTO sbh_user \
             (id, username, name, email, affiliation, password_hash, \
              graph_uri, is_admin, is_curator, is_member, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             RETURNING {USER_COLS}"
        ))
        .bind(id.as_uuid().to_string())
        .bind(&new_user.username)
        .bind(&new_user.name)
        .bind(&new_user.email)
        .bind(new_user.affiliation.as_deref())
        .bind(&new_user.password_hash)
        .bind(&new_user.graph_uri)
        .bind(new_user.is_admin as i64)
        .bind(new_user.is_curator as i64)
        .bind(new_user.is_member as i64)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        row_to_user(row)
    }

    async fn find_by_email_or_username(
        &self,
        identifier: &str,
    ) -> Result<Option<User>, DomainError> {
        let row = sqlx::query(&format!(
            "SELECT {USER_COLS} FROM sbh_user WHERE email = ? OR username = ? LIMIT 1"
        ))
        .bind(identifier)
        .bind(identifier)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(row_to_user).transpose()
    }

    async fn get_by_id(&self, id: UserId) -> Result<Option<User>, DomainError> {
        let row = sqlx::query(&format!("SELECT {USER_COLS} FROM sbh_user WHERE id = ?"))
            .bind(id.as_uuid().to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        row.map(row_to_user).transpose()
    }

    async fn update_user(&self, user: &User) -> Result<User, DomainError> {
        let row = sqlx::query(&format!(
            "UPDATE sbh_user \
                SET name = ?, affiliation = ?, \
                    is_admin = ?, is_curator = ?, is_member = ?, updated_at = ? \
              WHERE id = ? \
             RETURNING {USER_COLS}"
        ))
        .bind(&user.name)
        .bind(user.affiliation.as_deref())
        .bind(user.is_admin as i64)
        .bind(user.is_curator as i64)
        .bind(user.is_member as i64)
        .bind(Utc::now())
        .bind(user.id.as_uuid().to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?
        .ok_or_else(|| DomainError::NotFound(format!("user {}", user.id)))?;
        row_to_user(row)
    }

    async fn set_password_hash(&self, id: UserId, password_hash: &str) -> Result<(), DomainError> {
        sqlx::query("UPDATE sbh_user SET password_hash = ?, updated_at = ? WHERE id = ?")
            .bind(password_hash)
            .bind(Utc::now())
            .bind(id.as_uuid().to_string())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn set_reset_link(&self, id: UserId, link: Option<&str>) -> Result<(), DomainError> {
        sqlx::query("UPDATE sbh_user SET reset_password_link = ? WHERE id = ?")
            .bind(link)
            .bind(id.as_uuid().to_string())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn consume_reset_link(&self, link: &str) -> Result<Option<User>, DomainError> {
        let row = sqlx::query(&format!(
            "UPDATE sbh_user SET reset_password_link = NULL \
              WHERE reset_password_link = ? \
             RETURNING {USER_COLS}"
        ))
        .bind(link)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(row_to_user).transpose()
    }

    async fn delete_user(&self, id: UserId) -> Result<bool, DomainError> {
        let result = sqlx::query("DELETE FROM sbh_user WHERE id = ?")
            .bind(id.as_uuid().to_string())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(result.rows_affected() > 0)
    }
}

fn row_to_user(row: sqlx::sqlite::SqliteRow) -> Result<User, DomainError> {
    let id: String = row.try_get("id").map_err(db_err)?;
    Ok(User {
        id: UserId(uuid::Uuid::parse_str(&id).map_err(db_err)?),
        username: row.try_get("username").map_err(db_err)?,
        name: row.try_get("name").map_err(db_err)?,
        email: row.try_get("email").map_err(db_err)?,
        affiliation: row.try_get("affiliation").map_err(db_err)?,
        password_hash: row.try_get("password_hash").map_err(db_err)?,
        graph_uri: row.try_get("graph_uri").map_err(db_err)?,
        is_admin: row.try_get::<i64, _>("is_admin").map_err(db_err)? != 0,
        is_curator: row.try_get::<i64, _>("is_curator").map_err(db_err)? != 0,
        is_member: row.try_get::<i64, _>("is_member").map_err(db_err)? != 0,
        reset_password_link: row.try_get("reset_password_link").map_err(db_err)?,
        created_at: row.try_get("created_at").map_err(db_err)?,
        updated_at: row.try_get("updated_at").map_err(db_err)?,
    })
}
