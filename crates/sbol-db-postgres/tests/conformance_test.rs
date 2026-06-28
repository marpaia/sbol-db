//! Runs the backend-neutral `sbol-db-conformance` suite against the Postgres
//! backend. The same suite will pin the SQLite and RocksDB backends once they
//! exist, guaranteeing identical observable behavior across implementations.

use std::sync::Arc;

use sbol_db_postgres::{connect, run_migrations, JobRepository, SbolObjectService};
use sbol_db_storage::{JobQueue, SbolStore};

async fn fresh_handles() -> (Arc<dyn SbolStore>, Arc<dyn JobQueue>) {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://sbol:sbol@localhost:5432/sbol".to_owned());
    let pool = connect(&database_url).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    sqlx::query(
        "TRUNCATE sbol_graphs, sbol_objects, sbol_triples, sbol_validation_findings, \
         sbol_validation_runs, sbol_object_revisions, sbol_rdf_projection_events, sbol_components, \
         sbol_sequences, sbol_features, sbol_locations, sbol_constraints, \
         sbol_interactions, sbol_participations, sbol_sequence_kmers, sbol_ontologies, \
         sbol_ontology_terms, sbol_ontology_term_aliases, sbol_ontology_closure, \
         sbol_jobs, sbol_job_attempts, sbol_job_logs \
         RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate");

    let store: Arc<dyn SbolStore> = Arc::new(SbolObjectService::new(pool.clone()));
    let jobs: Arc<dyn JobQueue> = Arc::new(JobRepository::new(pool));
    (store, jobs)
}

#[tokio::test]
async fn postgres_passes_storage_conformance_suite() {
    let (store, jobs) = fresh_handles().await;
    sbol_db_conformance::run_all(store.as_ref(), jobs.as_ref()).await;
}
