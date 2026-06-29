//! Oxigraph persistence layer for sbol-db, exposed under the historical
//! `rocksdb://` scheme and `sbol-db-rocksdb` crate name.
//!
//! Triples and SPARQL are evaluated natively by Oxigraph's id-native engine
//! against a persistent RocksDB-backed store; the derived projections (objects,
//! graphs, ontology, sequences), the job queue, the lab dashboard, and the
//! SynBioHub query accelerator index live in a SQLite companion database. This
//! is an isolated experiment to benchmark Oxigraph against a hand-tuned
//! permuted-index triplestore.
//!
//! `rocksdb://<dir>` opens `<dir>/triples` (the Oxigraph store) and
//! `<dir>/companion.sqlite` (the companion, migrated on open).

mod accel;
mod convert;
mod db;
mod neighborhood;
mod sparql;
mod store;
mod triple_source;
mod triple_writer;

use sbol_db_core::DomainError;
use sbol_db_sqlite::{SqliteJobRepository, SqliteMigrator};

pub use db::OxigraphDb as Db;
pub use sparql::OxigraphNativeSparql;
pub use store::RocksdbStore;

/// Open (creating if absent) the stores named by a `rocksdb://<dir>` connection
/// string, migrating the SQLite companion. Async because the companion's open +
/// migrate is async.
pub async fn connect(conn: &str) -> Result<Db, DomainError> {
    Db::connect(conn).await
}

pub(crate) fn db_err<E: std::fmt::Display>(e: E) -> DomainError {
    DomainError::Database(e.to_string())
}

/// The job queue for the `rocksdb://` backend: the SQLite companion's job
/// repository over the companion pool.
pub struct RocksdbJobs;

impl RocksdbJobs {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(db: Db) -> SqliteJobRepository {
        SqliteJobRepository::new(db.pool)
    }
}

/// The migration capability for the `rocksdb://` backend: the SQLite companion's
/// migrator over the companion pool.
pub struct RocksdbMigrator;

impl RocksdbMigrator {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(db: Db) -> SqliteMigrator {
        SqliteMigrator::new(db.pool)
    }
}
