//! Unified storage model: the named graph owns its triples, and both ingest
//! routes (SBOL document import and verbatim Graph Store writes) register a
//! `sbol_graphs` row and write into it. These tests pin the lifecycle:
//!   * an imported document owns a `document`-kind graph; deleting the document
//!     drops the graph and its triples (cascade),
//!   * a Graph Store write owns an `rdf`-kind graph; clearing it removes both.

use std::sync::OnceLock;

use sbol_db_core::SerializationFormat;
use sbol_db_postgres::{connect, run_migrations, GraphWriteMode, ImportInput, SbolObjectService};
use sqlx::Row;
use tokio::sync::{Mutex, MutexGuard};

const FIXTURE: &str = include_str!("fixtures/simple_component.ttl");

static DB_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

async fn db_lock() -> MutexGuard<'static, ()> {
    DB_MUTEX.get_or_init(|| Mutex::new(())).lock().await
}

async fn fresh_service() -> SbolObjectService {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://sbol:sbol@localhost:5432/sbol".to_owned());
    let pool = connect(&database_url).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    sqlx::query(
        "TRUNCATE sbol_graphs, sbol_objects, sbol_triples, \
         sbol_validation_findings, sbol_validation_runs, sbol_object_revisions, \
         sbol_rdf_projection_events, sbol_components, sbol_sequences, sbol_features, \
         sbol_locations, sbol_constraints, sbol_interactions, sbol_participations \
         RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate");
    SbolObjectService::new(pool)
}

async fn graph_kind(svc: &SbolObjectService, iri: &str) -> Option<String> {
    sqlx::query("SELECT kind FROM sbol_graphs WHERE iri = $1")
        .bind(iri)
        .fetch_optional(svc.pool())
        .await
        .expect("query graph")
        .map(|r| r.get::<String, _>("kind"))
}

async fn triple_count(svc: &SbolObjectService, graph: &str) -> i64 {
    sqlx::query("SELECT count(*) AS n FROM sbol_triples WHERE graph_iri = $1")
        .bind(graph)
        .fetch_one(svc.pool())
        .await
        .expect("count triples")
        .get::<i64, _>("n")
}

#[tokio::test]
async fn imported_document_owns_a_graph_and_delete_cascades() {
    let _g = db_lock().await;
    let svc = fresh_service().await;

    let report = svc
        .import_document(ImportInput {
            body: FIXTURE.to_owned(),
            format: SerializationFormat::Turtle,
            namespace: None,
            source_uri: None,
            document_iri: None,
            created_by: None,
            name: None,
            description: None,
        })
        .await
        .expect("import");

    let graph = format!("graph:document:{}", report.graph_id.0);
    assert_eq!(
        graph_kind(&svc, &graph).await.as_deref(),
        Some("sbol3"),
        "import should register a document-kind graph"
    );
    assert!(
        triple_count(&svc, &graph).await > 0,
        "graph owns the triples"
    );

    // Deleting the document drops its graph, cascading the triples.
    assert!(
        svc.graphs().delete(report.graph_id).await.expect("delete"),
        "delete should remove something"
    );
    assert_eq!(
        graph_kind(&svc, &graph).await,
        None,
        "the document's graph is gone"
    );
    assert_eq!(
        triple_count(&svc, &graph).await,
        0,
        "triples cascade with the graph"
    );
}

#[tokio::test]
async fn graph_store_write_owns_an_rdf_graph_and_clear_removes_it() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    let graph = "https://synbiohub.org/public";

    let inserted = svc
        .graph_store_write(
            graph,
            "@prefix ex: <https://example.org/> .\nex:s ex:p ex:o .",
            SerializationFormat::Turtle,
            GraphWriteMode::Merge,
        )
        .await
        .expect("graph store write");
    assert_eq!(inserted, 1);
    assert_eq!(
        graph_kind(&svc, graph).await.as_deref(),
        Some("verbatim"),
        "a verbatim write registers an rdf-kind graph"
    );
    assert_eq!(triple_count(&svc, graph).await, 1);

    // Clearing the graph removes both its triples and its registry row.
    svc.graph_store_clear(graph).await.expect("clear");
    assert_eq!(graph_kind(&svc, graph).await, None, "graph row removed");
    assert_eq!(triple_count(&svc, graph).await, 0, "triples removed");
}
