//! Runs the `sbol-db-conformance` scenarios against the Oxigraph-backed
//! `rocksdb://` backend, the same contract the SQLite and Postgres backends
//! pass.

use sbol_db_rocksdb::{connect, Db, RocksdbJobs, RocksdbStore};
use tempfile::TempDir;

async fn fresh_db() -> (Db, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("conformance.rocksdb");
    let url = format!("rocksdb://{}", path.display());
    let db = connect(&url).await.expect("open rocksdb");
    (db, dir)
}

#[tokio::test]
async fn rocksdb_passes_import_and_read_back() {
    let (db, _dir) = fresh_db().await;
    sbol_db_conformance::import_and_read_back(&RocksdbStore::new(db)).await;
}

#[tokio::test]
async fn rocksdb_passes_graph_set_semantics() {
    let (db, _dir) = fresh_db().await;
    sbol_db_conformance::graph_set_semantics(&RocksdbStore::new(db)).await;
}

#[tokio::test]
async fn rocksdb_passes_ontology_roundtrip() {
    let (db, _dir) = fresh_db().await;
    sbol_db_conformance::ontology_roundtrip(&RocksdbStore::new(db)).await;
}

#[tokio::test]
async fn rocksdb_passes_neighborhood_walk() {
    let (db, _dir) = fresh_db().await;
    sbol_db_conformance::neighborhood_walk(&RocksdbStore::new(db)).await;
}

#[tokio::test]
async fn rocksdb_passes_sequence_search() {
    let (db, _dir) = fresh_db().await;
    sbol_db_conformance::sequence_search(&RocksdbStore::new(db)).await;
}

#[tokio::test]
async fn rocksdb_passes_job_queue_lifecycle() {
    let (db, _dir) = fresh_db().await;
    sbol_db_conformance::job_queue_lifecycle(&RocksdbJobs::new(db)).await;
}

#[tokio::test]
async fn rocksdb_passes_full_conformance_suite() {
    let (db, _dir) = fresh_db().await;
    sbol_db_conformance::run_all(&RocksdbStore::new(db.clone()), &RocksdbJobs::new(db)).await;
}
