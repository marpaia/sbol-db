//! Runs the `sbol-db-conformance` scenarios against the SQLite backend, the
//! same contract the Postgres and RocksDB backends pass.

use std::sync::Arc;

use sbol_db_app::AppServices;
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use sbol_db_sqlite::{
    connect_and_migrate, SqliteClusterStore, SqliteConfigStore, SqliteJobRepository,
    SqliteOAuthStore, SqlitePageRankStore, SqlitePool, SqliteSketchStore, SqliteStore,
    SqliteTokenStore, SqliteUserStore,
};
use sbol_db_storage::{
    AclStore, ClusterStore, ConfigStore, JobQueue, PageRankStore, SbolStore, SketchStore,
    TokenStore, UserStore,
};
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
    let store = Arc::new(SqliteStore::new(pool.clone()));
    let sparql = Arc::new(SparqlEngine::new(store.triple_source()));
    let sparql_update = Arc::new(SparqlUpdateEngine::new(
        store.triple_source(),
        store.triple_writer(),
    ));
    let jobs: Arc<dyn JobQueue> = Arc::new(SqliteJobRepository::new(pool.clone()));
    let store_dyn: Arc<dyn SbolStore> = store.clone();
    let acl: Arc<dyn AclStore> = store;
    let users: Arc<dyn UserStore> = Arc::new(SqliteUserStore::new(pool.clone()));
    let tokens: Arc<dyn TokenStore> = Arc::new(SqliteTokenStore::new(pool.clone()));
    let pagerank: Arc<dyn PageRankStore> = Arc::new(SqlitePageRankStore::new(pool.clone()));
    let cluster: Arc<dyn ClusterStore> = Arc::new(SqliteClusterStore::new(pool.clone()));
    let sketch: Arc<dyn SketchStore> = Arc::new(SqliteSketchStore::new(pool.clone()));
    let oauth = Arc::new(SqliteOAuthStore::new(pool.clone()));
    let config: Arc<dyn ConfigStore> = Arc::new(SqliteConfigStore::new(pool));
    let app = AppServices::new(store_dyn, sparql, sparql_update, jobs, acl)
        .with_identity(users, tokens)
        .with_oauth(oauth)
        .with_sequence_stores(pagerank, cluster, sketch)
        .with_config(config);
    sbol_db_conformance::run_all(&app).await;
}
