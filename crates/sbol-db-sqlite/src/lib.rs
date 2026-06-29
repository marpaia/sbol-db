//! SQLite persistence layer for sbol-db.
//!
//! Implements the `sbol-db-storage` contract over SQLite, mirroring the
//! Postgres model with portable SQL: the triplestore (with set semantics), the
//! graph registry, the derived object view, ontology loading, neighborhood
//! traversal, sequence search, the job queue, and the SynBioHub query
//! accelerator. The backend passes the full `sbol-db-conformance` suite.

pub mod pool;
mod repo;
mod store;

pub use pool::{connect, connect_and_migrate, run_migrations, SqliteMigrator, SqlitePool};
pub use repo::SqliteJobRepository;
pub use store::SqliteStore;
