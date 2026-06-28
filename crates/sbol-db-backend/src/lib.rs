//! Runtime storage-backend selection for sbol-db.
//!
//! [`Backend::open`] turns a connection string into a [`Backend`]: a
//! backend-neutral bundle of `sbol-db-storage` trait objects (the SBOL store,
//! the job queue, and the SPARQL read/write views). Consumers depend on the
//! trait objects, not on any concrete backend, so the same CLI and server
//! binary can drive whichever backend the connection string selects.
//!
//! The scheme picks the backend: `postgres://` / `postgresql://` is the only
//! one wired today; `sqlite://` and `rocksdb:` are planned. Features that are
//! irreducibly specific to one backend (Postgres introspection, the SQL
//! console) reach their backend through a typed handle such as
//! [`PostgresHandle`] rather than through the neutral surface.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use sbol_db_postgres::{JobRepository, PgMigrator, PgPool, PgStatsRepository, SbolObjectService};
use sbol_db_storage::{DbStats, JobQueue, Migrator, SbolStore, TripleSource, TripleWriter};

/// A ready-to-use storage backend: the neutral trait objects every consumer
/// shares, plus a typed handle for whichever concrete backend was opened.
pub struct Backend {
    /// The SBOL-aware store: ingest plus every derived-view read surface.
    pub store: Arc<dyn SbolStore>,
    /// The async job queue.
    pub jobs: Arc<dyn JobQueue>,
    /// Synchronous triple-pattern reads for the SPARQL evaluator.
    pub triple_source: Arc<dyn TripleSource>,
    /// Transactional triple writes for SPARQL Update.
    pub triple_writer: Arc<dyn TripleWriter>,
    /// Schema migrations, when the backend has a migratable schema.
    pub migrator: Option<Arc<dyn Migrator>>,
    /// Database introspection, when the backend can expose engine internals.
    pub db_stats: Option<Arc<dyn DbStats>>,
    /// Present when the Postgres backend is active; the seam through which
    /// the lab's irreducibly-Postgres SQL console reaches the pool.
    pub postgres: Option<PostgresHandle>,
}

/// Concrete handle to an open Postgres backend.
pub struct PostgresHandle {
    pub pool: PgPool,
    pub service: Arc<SbolObjectService>,
}

impl Backend {
    /// Open `conn` with no startup retry (fail fast on the first connection
    /// error). Suitable for short-lived CLI commands.
    pub async fn open(conn: &str) -> Result<Self> {
        Self::open_with_retry(conn, Duration::ZERO).await
    }

    /// Open `conn`, retrying connection failures until `deadline` elapses.
    /// A zero `deadline` fails fast. Suitable for daemons that race the
    /// database's own startup.
    pub async fn open_with_retry(conn: &str, deadline: Duration) -> Result<Self> {
        match backend_scheme(conn) {
            Some("postgres") | Some("postgresql") => {
                let pool = sbol_db_postgres::connect_with_retry(conn, deadline)
                    .await
                    .with_context(|| format!("connecting to {conn}"))?;
                Ok(Self::from_postgres(pool))
            }
            Some(other) => bail!(
                "unsupported storage backend scheme `{other}://` \
                 (supported: postgres://)"
            ),
            None => bail!(
                "connection string `{conn}` has no scheme; \
                 expected e.g. postgres://user:pass@host/db"
            ),
        }
    }

    /// Build a backend bundle over an already-open Postgres pool. Lets callers
    /// that tune their own pool (such as a dedicated worker pool) still obtain
    /// the neutral trait-object bundle.
    pub fn from_postgres_pool(pool: PgPool) -> Self {
        Self::from_postgres(pool)
    }

    fn from_postgres(pool: PgPool) -> Self {
        let service = Arc::new(SbolObjectService::new(pool.clone()));
        let triple_source = service.triple_source();
        let triple_writer = service.triple_writer();
        let jobs: Arc<dyn JobQueue> = Arc::new(JobRepository::new(pool.clone()));
        let migrator: Arc<dyn Migrator> = Arc::new(PgMigrator::new(pool.clone()));
        let db_stats: Arc<dyn DbStats> = Arc::new(PgStatsRepository::new(pool.clone()));
        let store: Arc<dyn SbolStore> = service.clone();
        Self {
            store,
            jobs,
            triple_source,
            triple_writer,
            migrator: Some(migrator),
            db_stats: Some(db_stats),
            postgres: Some(PostgresHandle { pool, service }),
        }
    }

    /// The Postgres handle, or an error when a different backend is active.
    /// Used by features that only make sense against Postgres.
    pub fn require_postgres(&self) -> Result<&PostgresHandle> {
        self.postgres
            .as_ref()
            .context("this operation requires the Postgres backend")
    }
}

/// The scheme of a connection string (`"postgres"` from
/// `postgres://...`), or `None` when there is no `://` separator.
fn backend_scheme(conn: &str) -> Option<&str> {
    conn.split_once("://").map(|(scheme, _)| scheme)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_schemes() {
        assert_eq!(
            backend_scheme("postgres://sbol:sbol@localhost/sbol"),
            Some("postgres")
        );
        assert_eq!(backend_scheme("sqlite:///tmp/x.db"), Some("sqlite"));
        assert_eq!(backend_scheme("nonsense"), None);
    }

    #[tokio::test]
    async fn rejects_unsupported_scheme() {
        let err = Backend::open("mysql://localhost/x").await.err().unwrap();
        assert!(err.to_string().contains("unsupported storage backend"));
    }

    #[tokio::test]
    async fn rejects_schemeless_connection_string() {
        let err = Backend::open("just-a-path").await.err().unwrap();
        assert!(err.to_string().contains("no scheme"));
    }
}
