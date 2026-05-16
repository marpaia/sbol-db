//! Integration tests for the SPARQL engine against the live docker-compose
//! Postgres. Mirrors the style of `crates/sbol-db-postgres/tests/neighborhood_test.rs`.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use sbol_db_core::SerializationFormat;
use sbol_db_postgres::{connect, run_migrations, ImportInput, SbolObjectService};
use sbol_db_sparql::{ResultFormat, SparqlEngine, SparqlError, SparqlOptions};
use tokio::sync::{Mutex, MutexGuard};

const NESTED: &str = include_str!("nested_construct.ttl");

const I13504: &str = "https://synbiohub.org/public/igem/i13504";
const SUBCOMP: &str = "https://synbiohub.org/public/igem/i13504/SubComponent1";

static DB_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

async fn db_lock() -> MutexGuard<'static, ()> {
    DB_MUTEX.get_or_init(|| Mutex::new(())).lock().await
}

struct Harness {
    engine: SparqlEngine,
    document_id: sbol_db_core::DocumentId,
}

async fn fresh_harness() -> Harness {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://sbol:sbol@localhost:5432/sbol".to_owned());
    let pool = connect(&database_url).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    sqlx::query(
        "TRUNCATE sbol_documents, sbol_objects, sbol_quads, sbol_validation_findings, \
         sbol_validation_runs, sbol_object_revisions, sbol_rdf_projection_events, sbol_components, \
         sbol_sequences, sbol_features, sbol_locations, sbol_constraints, \
         sbol_interactions, sbol_participations RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate");
    let svc = SbolObjectService::new(pool);
    let report = svc
        .import_document(ImportInput {
            body: NESTED.to_owned(),
            format: SerializationFormat::Turtle,
            source_uri: None,
            document_iri: None,
            created_by: None,
            name: None,
            description: None,
        })
        .await
        .expect("import");
    let engine = SparqlEngine::new(Arc::new(svc.quads().clone()));
    Harness {
        engine,
        document_id: report.document_id,
    }
}

fn long_options() -> SparqlOptions {
    SparqlOptions {
        timeout: Duration::from_secs(30),
        max_rows: 100_000,
        max_query_size: 64 * 1024,
    }
}

#[tokio::test]
async fn select_finds_components() {
    let _g = db_lock().await;
    let h = fresh_harness().await;
    let query = "PREFIX sbol: <http://sbols.org/v3#>\n\
                 SELECT ?s WHERE { ?s a sbol:Component }";
    let outcome = h
        .engine
        .execute(query, Some(ResultFormat::Json), &long_options())
        .await
        .expect("execute");
    let body = String::from_utf8(outcome.payload.body).expect("utf8");
    assert!(
        body.contains(I13504),
        "expected i13504 in solutions JSON, got: {body}"
    );
}

#[tokio::test]
async fn select_literal_filter_resolves_subcomponent() {
    let _g = db_lock().await;
    let h = fresh_harness().await;
    let query = "PREFIX sbol: <http://sbols.org/v3#>\n\
                 SELECT ?s WHERE { ?s sbol:displayId \"SubComponent1\" }";
    let outcome = h
        .engine
        .execute(query, Some(ResultFormat::Csv), &long_options())
        .await
        .expect("execute");
    let body = String::from_utf8(outcome.payload.body).expect("utf8");
    assert!(
        body.contains(SUBCOMP),
        "expected SubComponent IRI in CSV body, got: {body}"
    );
}

#[tokio::test]
async fn ask_true_when_features_exist() {
    let _g = db_lock().await;
    let h = fresh_harness().await;
    let query = "PREFIX sbol: <http://sbols.org/v3#>\n\
                 ASK { ?s sbol:hasFeature ?f }";
    let outcome = h
        .engine
        .execute(query, Some(ResultFormat::Json), &long_options())
        .await
        .expect("execute");
    let body = String::from_utf8(outcome.payload.body).expect("utf8");
    assert!(
        body.contains("\"boolean\":true"),
        "expected ASK true, got: {body}"
    );
}

#[tokio::test]
async fn construct_round_trips_through_turtle() {
    let _g = db_lock().await;
    let h = fresh_harness().await;
    let query = "PREFIX sbol: <http://sbols.org/v3#>\n\
                 CONSTRUCT { ?s sbol:displayId ?d }\n\
                 WHERE    { ?s sbol:displayId ?d }";
    let outcome = h
        .engine
        .execute(query, Some(ResultFormat::Turtle), &long_options())
        .await
        .expect("execute");
    let body = String::from_utf8(outcome.payload.body).expect("utf8");
    assert!(
        body.contains("displayId"),
        "expected displayId predicate in Turtle, got: {body}"
    );
    // sbol-rdf should accept the output.
    let _ = sbol_rdf::Graph::parse(&body, sbol_rdf::RdfFormat::Turtle).expect("re-parse Turtle");
}

#[tokio::test]
async fn describe_i13504_emits_fanout() {
    let _g = db_lock().await;
    let h = fresh_harness().await;
    let query = format!("DESCRIBE <{I13504}>", I13504 = I13504,);
    let outcome = h
        .engine
        .execute(&query, Some(ResultFormat::NTriples), &long_options())
        .await
        .expect("execute");
    let body = String::from_utf8(outcome.payload.body).expect("utf8");
    assert!(body.contains(I13504), "expected i13504 subject");
    assert!(body.contains(SUBCOMP), "expected SubComponent in fanout");
}

#[tokio::test]
async fn named_graph_scopes_results_to_one_document() {
    let _g = db_lock().await;
    let h = fresh_harness().await;
    let graph_iri = format!("graph:document:{}", h.document_id.0);
    let query = format!(
        "PREFIX sbol: <http://sbols.org/v3#>\n\
         SELECT ?s FROM NAMED <{graph_iri}>\n\
         WHERE {{ GRAPH <{graph_iri}> {{ ?s a sbol:Component }} }}",
    );
    let outcome = h
        .engine
        .execute(&query, Some(ResultFormat::Json), &long_options())
        .await
        .expect("execute");
    let body = String::from_utf8(outcome.payload.body).expect("utf8");
    assert!(
        body.contains(I13504),
        "expected i13504 in named-graph scoped query"
    );
}

#[tokio::test]
async fn update_strings_are_rejected() {
    let _g = db_lock().await;
    let h = fresh_harness().await;
    let result = h
        .engine
        .execute(
            "INSERT DATA { <http://example.com/a> <http://example.com/b> <http://example.com/c> }",
            Some(ResultFormat::Json),
            &long_options(),
        )
        .await;
    assert!(
        matches!(result, Err(SparqlError::UpdateNotAllowed)),
        "expected UpdateNotAllowed, got: {result:?}"
    );
}

#[tokio::test]
async fn unrelated_parse_errors_surface_as_parse() {
    let _g = db_lock().await;
    let h = fresh_harness().await;
    let result = h
        .engine
        .execute("not a valid query at all", None, &long_options())
        .await;
    assert!(
        matches!(result, Err(SparqlError::Parse(_))),
        "expected Parse error, got: {result:?}"
    );
}

#[tokio::test]
async fn unsupported_format_for_query_form_is_rejected() {
    let _g = db_lock().await;
    let h = fresh_harness().await;
    // SELECT can't be returned as Turtle.
    let result = h
        .engine
        .execute(
            "SELECT * WHERE { ?s ?p ?o }",
            Some(ResultFormat::Turtle),
            &long_options(),
        )
        .await;
    assert!(
        matches!(result, Err(SparqlError::UnsupportedFormat(_))),
        "expected UnsupportedFormat, got: {result:?}"
    );
}

#[tokio::test]
async fn format_negotiation_changes_body_shape() {
    let _g = db_lock().await;
    let h = fresh_harness().await;
    let query = "PREFIX sbol: <http://sbols.org/v3#>\n\
                 SELECT ?s WHERE { ?s a sbol:Component }";
    let json = h
        .engine
        .execute(query, Some(ResultFormat::Json), &long_options())
        .await
        .expect("json");
    let csv = h
        .engine
        .execute(query, Some(ResultFormat::Csv), &long_options())
        .await
        .expect("csv");
    let tsv = h
        .engine
        .execute(query, Some(ResultFormat::Tsv), &long_options())
        .await
        .expect("tsv");
    assert_eq!(json.payload.content_type, "application/sparql-results+json");
    assert_eq!(csv.payload.content_type, "text/csv");
    assert_eq!(tsv.payload.content_type, "text/tab-separated-values");
    assert_ne!(json.payload.body, csv.payload.body);
    assert_ne!(csv.payload.body, tsv.payload.body);
}
