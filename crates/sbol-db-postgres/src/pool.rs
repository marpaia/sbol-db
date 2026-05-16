use sbol_db_core::DomainError;

pub use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

fn db_err<E: std::fmt::Display>(e: E) -> DomainError {
    DomainError::Database(e.to_string())
}

pub async fn connect(database_url: &str) -> Result<PgPool, DomainError> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(database_url)
        .await
        .map_err(db_err)
}

pub async fn run_migrations(pool: &PgPool) -> Result<(), DomainError> {
    MIGRATOR.run(pool).await.map_err(db_err)
}

pub async fn migration_status(pool: &PgPool) -> Result<Vec<MigrationEntry>, DomainError> {
    let mut conn = pool.acquire().await.map_err(db_err)?;
    let applied = MIGRATOR
        .iter()
        .map(|m| (m.version, m.description.to_string()))
        .collect::<Vec<_>>();
    let applied_versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut *conn)
            .await
            .unwrap_or_default();
    Ok(applied
        .into_iter()
        .map(|(v, d)| MigrationEntry {
            version: v,
            description: d,
            applied: applied_versions.contains(&v),
        })
        .collect())
}

#[derive(Debug, Clone)]
pub struct MigrationEntry {
    pub version: i64,
    pub description: String,
    pub applied: bool,
}
