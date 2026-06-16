//! Spike S2: SPARQL UPDATE executor, validated against the real update
//! templates SynBioHub issues to Virtuoso's authenticated endpoint
//! (`~/git/SynBioHub/synbiohub/sparql/{UpdateMutableDescription,remove}.sparql`).
//!
//! Confirms the executor handles: `DELETE WHERE`, compound `;`-separated
//! operations, `INSERT DATA`, `DELETE DATA`, and `DELETE { ?s ?p ?o } WHERE
//! {...}`; that writes land in the `default-graph-uri` graph (Virtuoso
//! semantics); and that re-running an edit replaces rather than duplicates.

use std::sync::OnceLock;
use std::time::Duration;

use sbol_db_core::IriString;
use sbol_db_postgres::{connect, run_migrations, SbolObjectService};
use sbol_db_rdf::rdf_graph_to_triples;
use sbol_db_sparql::{ResultFormat, SparqlEngine, SparqlOptions, SparqlUpdateEngine};
use tokio::sync::{Mutex, MutexGuard};

const SBOL2: &str = include_str!("synbiohub_sbol2.ttl");
const PUBLIC_GRAPH: &str = "https://synbiohub.org/public";
const J23100: &str = "https://synbiohub.org/public/igem/BBa_J23100/1";
const B0034: &str = "https://synbiohub.org/public/igem/BBa_B0034/1";

static DB_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

async fn db_lock() -> MutexGuard<'static, ()> {
    DB_MUTEX.get_or_init(|| Mutex::new(())).lock().await
}

struct Harness {
    read: SparqlEngine,
    update: SparqlUpdateEngine,
}

async fn fresh_harness() -> Harness {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://sbol:sbol@localhost:5432/sbol".to_owned());
    let pool = connect(&database_url).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    sqlx::query(
        "TRUNCATE sbol_graphs, sbol_objects, sbol_triples, sbol_validation_findings, \
         sbol_validation_runs, sbol_object_revisions, sbol_rdf_projection_events, sbol_components, \
         sbol_sequences, sbol_features, sbol_locations, sbol_constraints, \
         sbol_interactions, sbol_participations RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate");

    let svc = SbolObjectService::new(pool.clone());
    let graph = sbol_rdf::Graph::parse(SBOL2, sbol_rdf::RdfFormat::Turtle).expect("parse sbol2");
    let triples = rdf_graph_to_triples(&graph, &IriString::unchecked(PUBLIC_GRAPH));
    {
        let mut conn = svc.pool().acquire().await.expect("acquire");
        svc.triples()
            .ensure_graph(&mut conn, PUBLIC_GRAPH, "verbatim")
            .await
            .expect("ensure graph");
        svc.triples()
            .insert_triples(&mut conn, &triples, "graph-store")
            .await
            .expect("seed");
    }
    Harness {
        read: SparqlEngine::new(svc.triple_source()),
        update: SparqlUpdateEngine::new(svc.triple_source(), svc.triple_writer()),
    }
}

fn opts() -> SparqlOptions {
    SparqlOptions {
        timeout: Duration::from_secs(30),
        max_rows: 100_000,
        max_query_size: 64 * 1024,
        default_graph: None,
    }
}

async fn select(read: &SparqlEngine, query: &str) -> String {
    let outcome = read
        .execute(query, Some(ResultFormat::Json), &opts())
        .await
        .expect("select");
    String::from_utf8(outcome.payload.body).expect("utf8")
}

/// classic `UpdateMutableDescription.sparql` (synbiohub3 variant): two
/// `DELETE WHERE` clauses then `INSERT DATA`, compound via `;`. Run twice to
/// confirm an edit replaces the prior value rather than accumulating.
#[tokio::test]
async fn update_mutable_description_replaces_in_place() {
    let _g = db_lock().await;
    let h = fresh_harness().await;

    let update_with = |desc: &str| {
        format!(
            r#"
PREFIX dcterms: <http://purl.org/dc/terms/>
PREFIX sbh: <http://wiki.synbiohub.org/wiki/Terms/synbiohub#>
DELETE WHERE {{ <{J23100}> sbh:mutableDescription ?desc . }} ;
DELETE WHERE {{ <{J23100}> dcterms:modified ?modified . }} ;
INSERT DATA {{
    <{J23100}> sbh:mutableDescription "{desc}" .
    <{J23100}> dcterms:modified "2026-06-15T00:00:00" .
}}
"#
        )
    };

    // First edit.
    let out = h
        .update
        .execute(
            &update_with("first description"),
            Some(PUBLIC_GRAPH),
            &opts(),
        )
        .await
        .expect("first update");
    assert_eq!(out.inserted, 2, "two triples inserted");
    assert_eq!(out.deleted, 0, "nothing to delete on first edit");

    let q = format!(
        r#"PREFIX sbh: <http://wiki.synbiohub.org/wiki/Terms/synbiohub#>
SELECT ?d WHERE {{ <{J23100}> sbh:mutableDescription ?d }}"#
    );
    let body = select(&h.read, &q).await;
    assert!(body.contains("first description"), "desc set: {body}");

    // The new triple must live in the default-graph-uri graph SynBioHub passed.
    let scoped = format!(
        r#"PREFIX sbh: <http://wiki.synbiohub.org/wiki/Terms/synbiohub#>
SELECT ?d FROM <{PUBLIC_GRAPH}> WHERE {{ <{J23100}> sbh:mutableDescription ?d }}"#
    );
    assert!(
        select(&h.read, &scoped).await.contains("first description"),
        "inserted triple should be in the public graph"
    );

    // Second edit: the prior description must be deleted, not duplicated.
    let out = h
        .update
        .execute(
            &update_with("second description"),
            Some(PUBLIC_GRAPH),
            &opts(),
        )
        .await
        .expect("second update");
    assert_eq!(out.deleted, 2, "old description + modified removed");
    assert_eq!(out.inserted, 2, "new description + modified inserted");

    let body = select(&h.read, &q).await;
    assert!(body.contains("second description"), "new desc present");
    assert!(
        !body.contains("first description"),
        "old desc gone (no duplicate): {body}"
    );
}

/// classic `remove.sparql`: `DELETE { ?s ?p ?o } WHERE { ?s ?p ?o . ?s
/// sbh:topLevel <uri> }`. Removes exactly the target top-level's triples and
/// leaves sibling top-levels intact.
#[tokio::test]
async fn remove_template_deletes_only_target_top_level() {
    let _g = db_lock().await;
    let h = fresh_harness().await;

    let remove = format!(
        r#"
PREFIX sbh: <http://wiki.synbiohub.org/wiki/Terms/synbiohub#>
DELETE {{ ?s ?p ?o }} WHERE {{
    ?s ?p ?o .
    ?s sbh:topLevel <{J23100}>
}}
"#
    );
    let out = h
        .update
        .execute(&remove, Some(PUBLIC_GRAPH), &opts())
        .await
        .expect("remove");
    assert!(out.deleted > 0, "should delete J23100's triples");

    // J23100 is gone.
    let j = format!(
        r#"PREFIX sbol2: <http://sbols.org/v2#>
SELECT ?p ?o WHERE {{ <{J23100}> ?p ?o }}"#
    );
    let body = select(&h.read, &j).await;
    assert!(
        !body.contains("BBa_J23100"),
        "J23100 triples should be gone: {body}"
    );

    // B0034 (a sibling top-level) is untouched.
    let b = format!(
        r#"PREFIX sbol2: <http://sbols.org/v2#>
SELECT ?d WHERE {{ <{B0034}> sbol2:displayId ?d }}"#
    );
    assert!(
        select(&h.read, &b).await.contains("BBa_B0034"),
        "sibling top-level must survive"
    );
}

/// `INSERT DATA` then `DELETE DATA` of the same triple round-trips cleanly and
/// reports accurate counts.
#[tokio::test]
async fn insert_data_then_delete_data() {
    let _g = db_lock().await;
    let h = fresh_harness().await;
    let triple = format!(r#"INSERT DATA {{ <{J23100}> <http://example.org/note> "hello" . }}"#);
    let out = h
        .update
        .execute(&triple, Some(PUBLIC_GRAPH), &opts())
        .await
        .expect("insert data");
    assert_eq!(out.inserted, 1);

    let q = format!(r#"SELECT ?o WHERE {{ <{J23100}> <http://example.org/note> ?o }}"#);
    assert!(select(&h.read, &q).await.contains("hello"), "note inserted");

    let del = format!(r#"DELETE DATA {{ <{J23100}> <http://example.org/note> "hello" . }}"#);
    let out = h
        .update
        .execute(&del, Some(PUBLIC_GRAPH), &opts())
        .await
        .expect("delete data");
    assert_eq!(out.deleted, 1);
    assert!(!select(&h.read, &q).await.contains("hello"), "note removed");
}
