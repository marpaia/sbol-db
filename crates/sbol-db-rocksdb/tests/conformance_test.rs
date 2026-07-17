//! Runs the `sbol-db-conformance` scenarios against the RocksDB backend, the
//! same contract the SQLite and Postgres backends pass.

use std::sync::Arc;

use sbol_db_app::AppServices;
use sbol_db_rocksdb::{
    connect, Db, RocksdbClusterStore, RocksdbConfigStore, RocksdbJobs, RocksdbPageRankStore,
    RocksdbStore, RocksdbTokenStore, RocksdbUserStore,
};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use sbol_db_storage::{
    AclStore, ClusterStore, ConfigStore, JobQueue, PageRankStore, SbolStore, TokenStore, UserStore,
};
use tempfile::TempDir;

fn fresh_db() -> (Db, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("conformance.rocksdb");
    let url = format!("rocksdb://{}", path.display());
    let db = connect(&url).expect("open rocksdb");
    (db, dir)
}

#[tokio::test]
async fn rocksdb_passes_import_and_read_back() {
    let (db, _dir) = fresh_db();
    sbol_db_conformance::import_and_read_back(&RocksdbStore::new(db)).await;
}

#[tokio::test]
async fn rocksdb_passes_graph_set_semantics() {
    let (db, _dir) = fresh_db();
    sbol_db_conformance::graph_set_semantics(&RocksdbStore::new(db)).await;
}

#[tokio::test]
async fn rocksdb_passes_ontology_roundtrip() {
    let (db, _dir) = fresh_db();
    sbol_db_conformance::ontology_roundtrip(&RocksdbStore::new(db)).await;
}

#[tokio::test]
async fn rocksdb_passes_neighborhood_walk() {
    let (db, _dir) = fresh_db();
    sbol_db_conformance::neighborhood_walk(&RocksdbStore::new(db)).await;
}

#[tokio::test]
async fn rocksdb_passes_sequence_search() {
    let (db, _dir) = fresh_db();
    sbol_db_conformance::sequence_search(&RocksdbStore::new(db)).await;
}

#[tokio::test]
async fn rocksdb_passes_job_queue_lifecycle() {
    let (db, _dir) = fresh_db();
    sbol_db_conformance::job_queue_lifecycle(&RocksdbJobs::new(db)).await;
}

#[tokio::test]
async fn rocksdb_passes_full_conformance_suite() {
    let (db, _dir) = fresh_db();
    let store = Arc::new(RocksdbStore::new(db.clone()));
    let sparql = Arc::new(SparqlEngine::new(store.triple_source()));
    let sparql_update = Arc::new(SparqlUpdateEngine::new(
        store.triple_source(),
        store.triple_writer(),
    ));
    let jobs: Arc<dyn JobQueue> = Arc::new(RocksdbJobs::new(db.clone()));
    let store_dyn: Arc<dyn SbolStore> = store.clone();
    let acl: Arc<dyn AclStore> = store;
    let users: Arc<dyn UserStore> = Arc::new(RocksdbUserStore::new(db.clone()));
    let tokens: Arc<dyn TokenStore> = Arc::new(RocksdbTokenStore::new(db.clone()));
    let pagerank: Arc<dyn PageRankStore> = Arc::new(RocksdbPageRankStore::new(db.clone()));
    let cluster: Arc<dyn ClusterStore> = Arc::new(RocksdbClusterStore::new(db.clone()));
    let config: Arc<dyn ConfigStore> = Arc::new(RocksdbConfigStore::new(db));
    let app = AppServices::new(store_dyn, sparql, sparql_update, jobs, acl)
        .with_identity(users, tokens)
        .with_sequence_stores(pagerank, cluster)
        .with_config(config);
    sbol_db_conformance::run_all(&app).await;
}
