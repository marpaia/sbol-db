//! RocksDB persistence layer for sbol-db.
//!
//! A dictionary-encoded, permuted-index triplestore tuned for single-node
//! performance: every RDF term is interned to a content-derived id, and a
//! triple is stored as a key in each permuted index so that any triple pattern
//! resolves to one prefix range scan and set semantics hold for free. The
//! derived views (objects, graphs, ontology, sequences, the job queue) are
//! hand-built over their own column families to match the contract the SQL
//! backends implement.

mod codec;
mod db;
mod jobs;
mod keys;
mod migrate;
mod repo;
mod store;

use std::path::Path;

use sbol_db_core::DomainError;

pub use db::Db;
pub use jobs::RocksdbJobs;
pub use migrate::RocksdbMigrator;
pub use store::RocksdbStore;

/// Open (creating if absent) the database named by a `rocksdb://<path>`
/// connection string. The column families are created on open, so the store is
/// usable immediately without a separate migration step.
pub fn connect(conn: &str) -> Result<Db, DomainError> {
    let path = conn
        .strip_prefix("rocksdb://")
        .or_else(|| conn.strip_prefix("rocksdb:"))
        .unwrap_or(conn);
    Db::open(Path::new(path))
}
