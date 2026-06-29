//! SQLite persistence layer for sbol-db.
//!
//! Implements the `sbol-db-storage` contract over SQLite, mirroring the
//! Postgres model with portable SQL. The triplestore (with set semantics), the
//! graph registry, and the derived object view are implemented; ontology
//! loading, neighborhood traversal, sequence search, and the job queue are not
//! yet ported.

pub mod pool;
pub mod repo;
mod store;

pub use pool::{connect, connect_and_migrate, run_migrations, SqliteMigrator, SqlitePool};
pub use repo::SqliteJobRepository;
pub use store::SqliteStore;
