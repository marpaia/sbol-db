//! Integration tests for ontology loading + closure queries.

use std::sync::OnceLock;

use sbol_db_postgres::{connect, run_migrations, SbolObjectService};
use tokio::sync::{Mutex, MutexGuard};

const TINY_SO: &str = include_str!("fixtures/tiny_so.obo");

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
        "TRUNCATE sbol_documents, sbol_objects, sbol_quads, validation_findings, \
         validation_runs, object_revisions, rdf_projection_events, sbol_components, \
         sbol_sequences, sbol_features, sbol_locations, sbol_constraints, \
         sbol_interactions, sbol_participations, sequence_kmers, ontologies, \
         ontology_terms, ontology_term_aliases, ontology_closure \
         RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate");
    SbolObjectService::new(pool)
}

async fn load_tiny_so(svc: &SbolObjectService) {
    svc.ontology()
        .load_from_text("SO", "Sequence Ontology (test)", None, TINY_SO)
        .await
        .expect("load tiny SO");
}

#[tokio::test]
async fn load_reports_term_and_closure_counts() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    let report = svc
        .ontology()
        .load_from_text("SO", "Sequence Ontology (test)", None, TINY_SO)
        .await
        .expect("load");
    assert_eq!(report.prefix, "SO");
    assert_eq!(report.term_count, 6);
    // Self-pairs + ancestor pairs. region->sequence_feature, ... promoter has
    // 3 ancestors plus self, so closure_count > 6.
    assert!(report.closure_count > report.term_count);
    // identifiers.org alias is one per term; alt_id gives one more.
    assert!(report.alias_count >= report.term_count);
}

#[tokio::test]
async fn descendants_of_region_include_all_subtypes() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    load_tiny_so(&svc).await;
    let region = "http://purl.obolibrary.org/obo/SO_0000001";
    let descendants = svc
        .ontology()
        .descendants(region)
        .await
        .expect("descendants");
    let iris: Vec<&str> = descendants.iter().map(|(i, _)| i.as_str()).collect();
    // Self
    assert!(iris.contains(&"http://purl.obolibrary.org/obo/SO_0000001"));
    // Direct children: cis_regulatory_region + terminator
    assert!(iris.contains(&"http://purl.obolibrary.org/obo/SO_0001055"));
    assert!(iris.contains(&"http://purl.obolibrary.org/obo/SO_0000141"));
    // Grandchildren / great-grandchildren: promoter + TATA_box
    assert!(iris.contains(&"http://purl.obolibrary.org/obo/SO_0000167"));
    assert!(iris.contains(&"http://purl.obolibrary.org/obo/SO_0000169"));
    // sequence_feature is the parent, not a descendant
    assert!(!iris.contains(&"http://purl.obolibrary.org/obo/SO_0000110"));
}

#[tokio::test]
async fn identifiers_org_alias_resolves_to_canonical() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    load_tiny_so(&svc).await;
    let alias = "http://identifiers.org/so/SO:0000167";
    let canonical = svc
        .ontology()
        .canonicalize(alias)
        .await
        .expect("canonicalize");
    assert_eq!(
        canonical.as_deref(),
        Some("http://purl.obolibrary.org/obo/SO_0000167")
    );
}

#[tokio::test]
async fn alt_id_resolves_to_replacement_term() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    load_tiny_so(&svc).await;
    // SO:0000167 has alt_id SO:0000067 in our fixture.
    let alt = "http://purl.obolibrary.org/obo/SO_0000067";
    let canonical = svc
        .ontology()
        .canonicalize(alt)
        .await
        .expect("canonicalize");
    assert_eq!(
        canonical.as_deref(),
        Some("http://purl.obolibrary.org/obo/SO_0000167")
    );
}

#[tokio::test]
async fn list_returns_loaded_ontology() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    load_tiny_so(&svc).await;
    let rows = svc.ontology().list_ontologies().await.expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].prefix, "SO");
    assert_eq!(rows[0].term_count, 6);
}

#[tokio::test]
async fn reload_replaces_previous_terms() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    load_tiny_so(&svc).await;
    load_tiny_so(&svc).await; // idempotent reload
    let rows = svc.ontology().list_ontologies().await.expect("list");
    assert_eq!(
        rows.len(),
        1,
        "reload should not duplicate the ontology row"
    );
    assert_eq!(rows[0].term_count, 6);
}

#[tokio::test]
async fn get_term_returns_metadata_and_resolves_aliases() {
    let _g = db_lock().await;
    let svc = fresh_service().await;
    load_tiny_so(&svc).await;
    let term = svc
        .ontology()
        .get_term("http://identifiers.org/so/SO:0000167")
        .await
        .expect("get_term")
        .expect("term present");
    assert_eq!(term.curie, "SO:0000167");
    assert_eq!(term.name, "promoter");
    assert_eq!(term.prefix, "SO");
}
