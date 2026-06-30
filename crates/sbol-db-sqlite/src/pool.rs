//! SQLite connection pool, schema migrations, and the [`Migrator`] capability.

use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use sbol_db_core::DomainError;
use sbol_db_storage::{MigrationEntry, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

pub use sqlx::SqlitePool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub(crate) fn db_err<E: std::fmt::Display>(e: E) -> DomainError {
    DomainError::Database(e.to_string())
}

/// Open a SQLite pool for `url` (e.g. `sqlite://data/sbol.db` or
/// `sqlite::memory:`). WAL mode plus a busy timeout let readers and a single
/// writer coexist without spurious "database is locked" errors; foreign keys
/// are enabled so the graph→triples cascade fires.
pub async fn connect(url: &str) -> Result<SqlitePool, DomainError> {
    let options = SqliteConnectOptions::from_str(url)
        .map_err(db_err)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);
    SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await
        .map_err(db_err)
}

/// Open `url` and apply every pending migration.
pub async fn connect_and_migrate(url: &str) -> Result<SqlitePool, DomainError> {
    let pool = connect(url).await?;
    run_migrations(&pool).await?;
    Ok(pool)
}

pub async fn run_migrations(pool: &SqlitePool) -> Result<(), DomainError> {
    MIGRATOR.run(pool).await.map_err(db_err)
}

pub async fn migration_status(pool: &SqlitePool) -> Result<Vec<MigrationEntry>, DomainError> {
    let known = MIGRATOR
        .iter()
        .map(|m| (m.version, m.description.to_string()))
        .collect::<Vec<_>>();
    let applied: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    Ok(known
        .into_iter()
        .map(|(version, description)| MigrationEntry {
            version,
            description,
            applied: applied.contains(&version),
        })
        .collect())
}

/// The migration capability for a SQLite backend.
#[derive(Clone)]
pub struct SqliteMigrator {
    pool: SqlitePool,
}

impl SqliteMigrator {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl Migrator for SqliteMigrator {
    async fn run_migrations(&self) -> Result<(), DomainError> {
        run_migrations(&self.pool).await
    }

    async fn migration_status(&self) -> Result<Vec<MigrationEntry>, DomainError> {
        migration_status(&self.pool).await
    }
}
