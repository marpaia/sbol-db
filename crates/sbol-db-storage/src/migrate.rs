//! Schema-migration capability.
//!
//! Every backend brings its schema up to date its own way (SQL migration
//! files, programmatic setup, or nothing at all for a schemaless store), so
//! migration is a capability rather than part of the core store contract.

use async_trait::async_trait;
use sbol_db_core::DomainError;

/// One migration and whether it has been applied to the open database.
#[derive(Clone, Debug)]
pub struct MigrationEntry {
    pub version: i64,
    pub description: String,
    pub applied: bool,
}

/// Bring a backend's schema up to date and report migration state.
#[async_trait]
pub trait Migrator: Send + Sync {
    /// Apply every pending migration.
    async fn run_migrations(&self) -> Result<(), DomainError>;
    /// List every known migration with its applied state.
    async fn migration_status(&self) -> Result<Vec<MigrationEntry>, DomainError>;
}
