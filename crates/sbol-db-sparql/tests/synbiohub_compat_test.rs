//! Spikes S1 (spareval feature coverage) and S3 (verbatim round-trip fidelity)
//! for the SynBioHub Virtuoso-compat work.
//!
//! These load a SynBioHub-shaped SBOL2 graph **verbatim** (no SBOL2->3 upgrade,
//! no projection) via the new `TripleRepository::insert_triples` path, then run the
//! real SPARQL shapes SynBioHub issues against Virtuoso. The point is to prove,
//! before building the protocol surface, that:
//!   * spareval evaluates the features SynBioHub depends on (DISTINCT, OPTIONAL,
//!     UNION, sub-SELECT, FILTER + STRSTARTS/CONTAINS/str()/lcase, FILTER NOT
//!     EXISTS, COUNT aggregation, CONSTRUCT), and
//!   * the verbatim store round-trips SBOL2 triples byte-for-triple (v2 IRIs,
//!     literals, datatypes preserved).
//!
//! Queries are adapted from `~/git/SynBioHub/synbiohub/sparql/*.sparql` and the
//! inline `CONTAINS` criteria in `lib/search.js`, with template placeholders
//! filled in. The default graph is the union of named graphs (the engine sets
//! `set_default_graph_as_union()`), so `default-graph-uri` handling (Phase 1)
//! is not needed here.

use std::sync::OnceLock;
use std::time::Duration;

use sbol_db_core::IriString;
use sbol_db_postgres::{connect, run_migrations, SbolObjectService};
use sbol_db_rdf::rdf_graph_to_triples;
use sbol_db_sparql::{ResultFormat, SparqlEngine, SparqlOptions};
use tokio::sync::{Mutex, MutexGuard};

const SBOL2: &str = include_str!("synbiohub_sbol2.ttl");
const PUBLIC_GRAPH: &str = "https://synbiohub.org/public";

const COLLECTION: &str = "https://synbiohub.org/public/igem/igem_collection/1";
const J23100: &str = "https://synbiohub.org/public/igem/BBa_J23100/1";
const J23100_SEQ: &str = "https://synbiohub.org/public/igem/BBa_J23100_sequence/1";
const B0034: &str = "https://synbiohub.org/public/igem/BBa_B0034/1";

static DB_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

async fn db_lock() -> MutexGuard<'static, ()> {
    DB_MUTEX.get_or_init(|| Mutex::new(())).lock().await
}

/// Truncate, then load the SBOL2 fixture verbatim into the public named graph
/// via `insert_triples` (graph_id = NULL, source = "graph-store"). Returns an
/// engine over the resulting store.
async fn fresh_engine() -> SparqlEngine {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://sbol:sbol@localhost:5432/sbol".to_owned());
    let pool = connect(&database_url).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    sqlx::query(
        "TRUNCATE sbol_graphs, sbol_objects, sbol_triples, sbol_validation_findings, \
         sbol_validation_runs, sbol_object_revisions, sbol_rdf_projection_events, sbol_components, \
         sbol_sequences, sbol_features, sbol_locations, sbol_constraints, \
         sbol_interactions, sbol_participations, accel_dirty, accel_object, accel_type, \
         accel_member, accel_facet RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate");

    let svc = SbolObjectService::new(pool);

    // Parse the SBOL2 RDF as a *generic* graph (no SBOL interpretation) and
    // store it verbatim in a registered named graph.
    let graph = sbol_rdf::Graph::parse(SBOL2, sbol_rdf::RdfFormat::Turtle).expect("parse sbol2");
    let triples = rdf_graph_to_triples(&graph, &IriString::unchecked(PUBLIC_GRAPH));
    let inserted = {
        let mut conn = svc.pool().acquire().await.expect("acquire");
        svc.triples()
            .ensure_graph(&mut conn, PUBLIC_GRAPH, "verbatim")
            .await
            .expect("ensure_graph");
        let n = svc
            .triples()
            .insert_triples(&mut conn, &triples, "graph-store")
            .await
            .expect("insert_triples");
        // The raw insert bypasses the Graph Store write path, so refresh the
        // accelerator by hand (the write path does this within its transaction).
        svc.accel()
            .refresh_graph(&mut conn, PUBLIC_GRAPH)
            .await
            .expect("refresh accel");
        n
    };
    assert_eq!(
        inserted,
        graph.triples().len(),
        "every parsed triple should be stored verbatim"
    );

    SparqlEngine::new(svc.triple_source())
}

fn long_options() -> SparqlOptions {
    SparqlOptions {
        timeout: Duration::from_secs(30),
        max_rows: 100_000,
        max_query_size: 64 * 1024,
        default_graph: None,
    }
}

async fn run(engine: &SparqlEngine, query: &str, format: ResultFormat) -> String {
    let outcome = engine
        .execute(query, Some(format), &long_options())
        .await
        .unwrap_or_else(|e| panic!("execute failed for query:\n{query}\nerror: {e:?}"));
    String::from_utf8(outcome.payload.body).expect("utf8")
}

/// classic `search.sparql` with empty `$criteria`/`$from`/`$limit`/`$offset`:
/// DISTINCT + a self-edge `sbh:topLevel` + OPTIONAL fan-out + FILTER STRSTARTS.
#[tokio::test]
async fn synbiohub_search_template_evaluates() {
    let _g = db_lock().await;
    let engine = fresh_engine().await;
    let query = r#"
PREFIX sbol2: <http://sbols.org/v2#>
PREFIX dcterms: <http://purl.org/dc/terms/>
PREFIX sbh: <http://wiki.synbiohub.org/wiki/Terms/synbiohub#>
SELECT DISTINCT ?subject ?displayId ?version ?name ?description ?type ?sbolType ?role
WHERE {
    ?subject a ?type .
    ?subject sbh:topLevel ?subject .
    OPTIONAL { ?subject sbol2:displayId ?displayId . }
    OPTIONAL { ?subject sbol2:version ?version . }
    OPTIONAL { ?subject dcterms:title ?name . }
    OPTIONAL { ?subject dcterms:description ?description . }
    OPTIONAL { ?subject sbol2:type ?sbolType . FILTER(STRSTARTS(str(?sbolType),'http://www.biopax.org/release/biopax-level3.owl')) }
    OPTIONAL { ?subject sbol2:role ?role . FILTER(STRSTARTS(str(?role),'http://identifiers.org/so/')) }
}
"#;
    let body = run(&engine, query, ResultFormat::Json).await;
    // All four topLevels surface.
    for iri in [COLLECTION, J23100, J23100_SEQ, B0034] {
        assert!(
            body.contains(iri),
            "expected {iri} in search results: {body}"
        );
    }
    // The biopax type IRI and SO role IRI came back verbatim (S3).
    assert!(
        body.contains("http://www.biopax.org/release/biopax-level3.owl#DnaRegion"),
        "expected verbatim biopax type IRI: {body}"
    );
    assert!(
        body.contains("http://identifiers.org/so/0000167"),
        "expected verbatim SO role IRI: {body}"
    );
}

/// The inline full-text criteria from `lib/search.js`: CONTAINS(lcase(..), ..)
/// across displayId/name, OR-combined. Standard SPARQL 1.1, not bif:contains.
#[tokio::test]
async fn synbiohub_contains_fulltext_filter() {
    let _g = db_lock().await;
    let engine = fresh_engine().await;
    let query = r#"
PREFIX sbol2: <http://sbols.org/v2#>
PREFIX dcterms: <http://purl.org/dc/terms/>
PREFIX sbh: <http://wiki.synbiohub.org/wiki/Terms/synbiohub#>
SELECT DISTINCT ?subject WHERE {
    ?subject a ?type .
    ?subject sbh:topLevel ?subject .
    OPTIONAL { ?subject sbol2:displayId ?displayId . }
    OPTIONAL { ?subject dcterms:title ?name . }
    FILTER(CONTAINS(lcase(?displayId), lcase("j23100")) || CONTAINS(lcase(?name), lcase("j23100")))
}
"#;
    let body = run(&engine, query, ResultFormat::Json).await;
    assert!(body.contains(J23100), "expected J23100 match: {body}");
    assert!(
        !body.contains(B0034),
        "B0034 should not match j23100: {body}"
    );
}

/// synbiohub3 `RootCollectionMetadata.sparql`: FILTER NOT EXISTS.
#[tokio::test]
async fn synbiohub_root_collection_filter_not_exists() {
    let _g = db_lock().await;
    let engine = fresh_engine().await;
    let query = r#"
PREFIX sbol2: <http://sbols.org/v2#>
SELECT ?Collection WHERE {
    ?Collection a sbol2:Collection .
    FILTER NOT EXISTS { ?otherCollection sbol2:member ?Collection }
}
"#;
    let body = run(&engine, query, ResultFormat::Json).await;
    assert!(
        body.contains(COLLECTION),
        "root collection should surface: {body}"
    );
}

/// `CountMembers.sparql`-style aggregation: COUNT(DISTINCT ...).
#[tokio::test]
async fn synbiohub_count_members_aggregation() {
    let _g = db_lock().await;
    let engine = fresh_engine().await;
    let query = format!(
        r#"
PREFIX sbol2: <http://sbols.org/v2#>
SELECT (COUNT(DISTINCT ?member) AS ?count) WHERE {{
    <{COLLECTION}> sbol2:member ?member .
}}
"#
    );
    let body = run(&engine, &query, ResultFormat::Json).await;
    assert!(
        body.contains("\"value\":\"2\""),
        "expected member count of 2: {body}"
    );
}

/// synbiohub3 `FetchSBOLRecursive.sparql`: CONSTRUCT over a sub-SELECT with a
/// UNION and ORDER BY. Exercises the recursive-fetch shape and (S3) confirms
/// the verbatim sequence literal survives the round-trip.
#[tokio::test]
async fn synbiohub_fetch_recursive_construct() {
    let _g = db_lock().await;
    let engine = fresh_engine().await;
    let query = format!(
        r#"
PREFIX sbol: <http://sbols.org/v2#>
PREFIX sbh: <http://wiki.synbiohub.org/wiki/Terms/synbiohub#>
CONSTRUCT {{ ?s ?p ?o }} WHERE {{
    {{
        SELECT DISTINCT ?s WHERE {{
            {{ ?s sbh:topLevel <{J23100}> . }}
            UNION
            {{ <{J23100}> sbol:sequence ?s . }}
        }}
        ORDER BY ?s
    }}
    ?s ?p ?o
}}
"#
    );
    let body = run(&engine, &query, ResultFormat::NTriples).await;
    assert!(body.contains(J23100), "expected J23100 triples: {body}");
    assert!(
        body.contains(J23100_SEQ),
        "expected linked sequence via UNION: {body}"
    );
    // S3: the nucleotide elements literal came back exactly as stored.
    assert!(
        body.contains("ttgacggctagctcagtcctaggtacagtgctagc"),
        "expected verbatim sequence elements literal: {body}"
    );
}

/// Phase 1: the `default-graph-uri` protocol parameter scopes reads to one
/// named graph (Virtuoso semantics SynBioHub relies on for public vs per-user
/// graph isolation).
#[tokio::test]
async fn default_graph_uri_scopes_reads() {
    const USER_GRAPH: &str = "https://synbiohub.org/user/alice";
    let _g = db_lock().await;

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://sbol:sbol@localhost:5432/sbol".to_owned());
    let pool = connect(&database_url).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    sqlx::query(
        "TRUNCATE sbol_graphs, sbol_triples, accel_dirty, accel_object, accel_type, \
         accel_member, accel_facet RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate");
    let svc = SbolObjectService::new(pool);

    // Public graph: the SBOL2 fixture. User graph: one unrelated collection.
    let public = sbol_rdf::Graph::parse(SBOL2, sbol_rdf::RdfFormat::Turtle).expect("parse public");
    let public_triples = rdf_graph_to_triples(&public, &IriString::unchecked(PUBLIC_GRAPH));
    let user = sbol_rdf::Graph::parse(
        "@prefix sbol2: <http://sbols.org/v2#> .\n\
         <https://synbiohub.org/user/alice/proj/1> a sbol2:Collection .",
        sbol_rdf::RdfFormat::Turtle,
    )
    .expect("parse user");
    let user_triples = rdf_graph_to_triples(&user, &IriString::unchecked(USER_GRAPH));
    {
        let mut conn = svc.pool().acquire().await.expect("acquire");
        svc.triples()
            .ensure_graph(&mut conn, PUBLIC_GRAPH, "verbatim")
            .await
            .expect("ensure public");
        svc.triples()
            .ensure_graph(&mut conn, USER_GRAPH, "verbatim")
            .await
            .expect("ensure user");
        svc.triples()
            .insert_triples(&mut conn, &public_triples, "graph-store")
            .await
            .expect("seed public");
        svc.triples()
            .insert_triples(&mut conn, &user_triples, "graph-store")
            .await
            .expect("seed user");
        // The raw inserts bypass the Graph Store write path, so refresh the
        // accelerator by hand (the write path does this within its transaction).
        svc.accel()
            .refresh_graph(&mut conn, PUBLIC_GRAPH)
            .await
            .expect("refresh public accel");
        svc.accel()
            .refresh_graph(&mut conn, USER_GRAPH)
            .await
            .expect("refresh user accel");
    }
    let engine = SparqlEngine::new(svc.triple_source());

    let query = "PREFIX sbol2: <http://sbols.org/v2#>\n\
                 SELECT ?c WHERE { ?c a sbol2:Collection }";
    let scoped = |graph: &str| SparqlOptions {
        default_graph: Some(graph.to_owned()),
        ..long_options()
    };

    // Scoped to the public graph: the iGEM collection, not Alice's.
    let body = String::from_utf8(
        engine
            .execute(query, Some(ResultFormat::Json), &scoped(PUBLIC_GRAPH))
            .await
            .expect("public")
            .payload
            .body,
    )
    .unwrap();
    assert!(body.contains("igem_collection"), "public scope: {body}");
    assert!(
        !body.contains("user/alice"),
        "must not leak user graph: {body}"
    );

    // Scoped to Alice's graph: only her collection.
    let body = String::from_utf8(
        engine
            .execute(query, Some(ResultFormat::Json), &scoped(USER_GRAPH))
            .await
            .expect("user")
            .payload
            .body,
    )
    .unwrap();
    assert!(body.contains("user/alice/proj/1"), "user scope: {body}");
    assert!(
        !body.contains("igem_collection"),
        "must not leak public graph: {body}"
    );
}
