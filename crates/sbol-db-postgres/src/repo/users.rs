//! The account store over Postgres.
//!
//! Rows map to [`User`]; the unique `username` and `email` constraints are
//! enforced by the schema. Password hashing lives in the application layer, so
//! these methods store and read an already-computed `password_hash`.

use async_trait::async_trait;
use chrono::Utc;
use sbol_db_core::{DomainError, NewUser, User, UserId};
use sbol_db_storage::UserStore;
use sqlx::Row;
use uuid::Uuid;

use crate::repo::db_err;
use crate::PgPool;

const USER_COLS: &str = "id, username, name, email, affiliation, password_hash, \
    graph_uri, is_admin, is_curator, is_member, reset_password_link, created_at, updated_at";

#[derive(Clone)]
pub struct PgUserStore {
    pool: PgPool,
}

impl PgUserStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl UserStore for PgUserStore {
    async fn create_user(&self, new_user: NewUser) -> Result<User, DomainError> {
        let id = UserId::new();
        let now = Utc::now();
        let row = sqlx::query(&format!(
            "INSERT INTO sbh_user \
             (id, username, name, email, affiliation, password_hash, \
              graph_uri, is_admin, is_curator, is_member, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
             RETURNING {USER_COLS}"
        ))
        .bind(id.as_uuid())
        .bind(&new_user.username)
        .bind(&new_user.name)
        .bind(&new_user.email)
        .bind(new_user.affiliation.as_deref())
        .bind(&new_user.password_hash)
        .bind(&new_user.graph_uri)
        .bind(new_user.is_admin)
        .bind(new_user.is_curator)
        .bind(new_user.is_member)
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
            "SELECT {USER_COLS} FROM sbh_user WHERE email = $1 OR username = $1 LIMIT 1"
        ))
        .bind(identifier)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(row_to_user).transpose()
    }

    async fn get_by_id(&self, id: UserId) -> Result<Option<User>, DomainError> {
        let row = sqlx::query(&format!("SELECT {USER_COLS} FROM sbh_user WHERE id = $1"))
            .bind(id.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        row.map(row_to_user).transpose()
    }

    async fn update_user(&self, user: &User) -> Result<User, DomainError> {
        let row = sqlx::query(&format!(
            "UPDATE sbh_user \
                SET name = $2, affiliation = $3, \
                    is_admin = $4, is_curator = $5, is_member = $6, updated_at = $7 \
              WHERE id = $1 \
             RETURNING {USER_COLS}"
        ))
        .bind(user.id.as_uuid())
        .bind(&user.name)
        .bind(user.affiliation.as_deref())
        .bind(user.is_admin)
        .bind(user.is_curator)
        .bind(user.is_member)
        .bind(Utc::now())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?
        .ok_or_else(|| DomainError::NotFound(format!("user {}", user.id)))?;
        row_to_user(row)
    }

    async fn set_password_hash(&self, id: UserId, password_hash: &str) -> Result<(), DomainError> {
        sqlx::query("UPDATE sbh_user SET password_hash = $2, updated_at = $3 WHERE id = $1")
            .bind(id.as_uuid())
            .bind(password_hash)
            .bind(Utc::now())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn set_reset_link(&self, id: UserId, link: Option<&str>) -> Result<(), DomainError> {
        sqlx::query("UPDATE sbh_user SET reset_password_link = $2 WHERE id = $1")
            .bind(id.as_uuid())
            .bind(link)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn consume_reset_link(&self, link: &str) -> Result<Option<User>, DomainError> {
        let row = sqlx::query(&format!(
            "UPDATE sbh_user SET reset_password_link = NULL \
              WHERE reset_password_link = $1 \
             RETURNING {USER_COLS}"
        ))
        .bind(link)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(row_to_user).transpose()
    }

    async fn delete_user(&self, id: UserId) -> Result<bool, DomainError> {
        let result = sqlx::query("DELETE FROM sbh_user WHERE id = $1")
            .bind(id.as_uuid())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(result.rows_affected() > 0)
    }

    async fn any_admin(&self) -> Result<bool, DomainError> {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sbh_user WHERE is_admin = true)")
                .fetch_one(&self.pool)
                .await
                .map_err(db_err)?;
        Ok(exists)
    }
}

fn row_to_user(row: sqlx::postgres::PgRow) -> Result<User, DomainError> {
    let id: Uuid = row.try_get("id").map_err(db_err)?;
    Ok(User {
        id: UserId(id),
        username: row.try_get("username").map_err(db_err)?,
        name: row.try_get("name").map_err(db_err)?,
        email: row.try_get("email").map_err(db_err)?,
        affiliation: row.try_get("affiliation").map_err(db_err)?,
        password_hash: row.try_get("password_hash").map_err(db_err)?,
        graph_uri: row.try_get("graph_uri").map_err(db_err)?,
        is_admin: row.try_get("is_admin").map_err(db_err)?,
        is_curator: row.try_get("is_curator").map_err(db_err)?,
        is_member: row.try_get("is_member").map_err(db_err)?,
        reset_password_link: row.try_get("reset_password_link").map_err(db_err)?,
        created_at: row.try_get("created_at").map_err(db_err)?,
        updated_at: row.try_get("updated_at").map_err(db_err)?,
    })
}
