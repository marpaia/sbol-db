//! The composed handle for the Oxigraph-backed `rocksdb://` backend: an
//! Oxigraph [`Store`] for triples and native SPARQL, plus a SQLite companion
//! pool for the derived projections, the job queue, and the SynBioHub
//! accelerator index.
//!
//! `rocksdb://<dir>` opens `<dir>/triples` as the Oxigraph store (a persistent,
//! exclusively-locked RocksDB instance owned by Oxigraph) and
//! `<dir>/companion.sqlite` as the companion, migrated on open. There is no
//! cross-engine transaction: writes land in Oxigraph first, then the companion;
//! the companion's graph row is the commit witness and re-inserting identical
//! quads is idempotent.

use std::path::Path;

use oxigraph::store::Store;
use sbol_db_core::DomainError;
use sbol_db_sqlite::SqlitePool;

/// The two stores behind one `rocksdb://` connection. Cloneable; all clones
/// share the one Oxigraph handle and the one SQLite pool.
#[derive(Clone)]
pub struct OxigraphDb {
    pub store: Store,
    pub pool: SqlitePool,
}

impl OxigraphDb {
    /// Open (creating if absent) the stores under the directory named by a
    /// `rocksdb://<dir>` connection string, migrating the SQLite companion.
    pub async fn connect(conn: &str) -> Result<Self, DomainError> {
        let dir = conn
            .strip_prefix("rocksdb://")
            .or_else(|| conn.strip_prefix("rocksdb:"))
            .unwrap_or(conn);
        let dir = Path::new(dir);
        std::fs::create_dir_all(dir)
            .map_err(|e| DomainError::Database(format!("create {}: {e}", dir.display())))?;

        let triples_path = dir.join("triples");
        let store = Store::open(&triples_path)
            .map_err(|e| DomainError::Database(format!("open oxigraph store: {e}")))?;

        let companion_path = dir.join("companion.sqlite");
        let companion_url = format!("sqlite://{}", companion_path.display());
        let pool = sbol_db_sqlite::connect_and_migrate(&companion_url).await?;

        Ok(Self { store, pool })
    }
}
