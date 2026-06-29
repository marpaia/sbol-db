//! Runs the implemented `sbol-db-conformance` scenarios against the SQLite
//! backend. Ontology, job-queue, neighborhood, and sequence-search scenarios
//! are added here as those surfaces are ported.

use sbol_db_sqlite::{connect_and_migrate, SqliteJobRepository, SqlitePool, SqliteStore};
use tempfile::TempDir;

async fn fresh_pool() -> (SqlitePool, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("conformance.db");
    let url = format!("sqlite://{}", path.display());
    let pool = connect_and_migrate(&url).await.expect("connect + migrate");
    (pool, dir)
}

#[tokio::test]
async fn sqlite_passes_import_and_read_back() {
    let (pool, _dir) = fresh_pool().await;
    sbol_db_conformance::import_and_read_back(&SqliteStore::new(pool)).await;
}

#[tokio::test]
async fn sqlite_passes_graph_set_semantics() {
    let (pool, _dir) = fresh_pool().await;
    sbol_db_conformance::graph_set_semantics(&SqliteStore::new(pool)).await;
}

#[tokio::test]
async fn sqlite_passes_ontology_roundtrip() {
    let (pool, _dir) = fresh_pool().await;
    sbol_db_conformance::ontology_roundtrip(&SqliteStore::new(pool)).await;
}

#[tokio::test]
async fn sqlite_passes_neighborhood_walk() {
    let (pool, _dir) = fresh_pool().await;
    sbol_db_conformance::neighborhood_walk(&SqliteStore::new(pool)).await;
}

#[tokio::test]
async fn sqlite_passes_sequence_search() {
    let (pool, _dir) = fresh_pool().await;
    sbol_db_conformance::sequence_search(&SqliteStore::new(pool)).await;
}

#[tokio::test]
async fn sqlite_passes_job_queue_lifecycle() {
    let (pool, _dir) = fresh_pool().await;
    sbol_db_conformance::job_queue_lifecycle(&SqliteJobRepository::new(pool)).await;
}

#[tokio::test]
async fn sqlite_passes_full_conformance_suite() {
    let (pool, _dir) = fresh_pool().await;
    sbol_db_conformance::run_all(
        &SqliteStore::new(pool.clone()),
        &SqliteJobRepository::new(pool),
    )
    .await;
}
