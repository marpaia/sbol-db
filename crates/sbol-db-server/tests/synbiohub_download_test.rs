//! HTTP-level integration test for the SynBioHub V1 download routes.
//!
//! Seeds one public ComponentDefinition and its Sequence (SBOL2, the vocabulary
//! SynBioHub stores) into the public graph, then drives the real axum router
//! over a SQLite-backed [`AppState`]: `GET <uri>/sbol` must return the object's
//! RDF closure and `GET <uri>/fasta` must return its sequence residues. The
//! FASTA path exercises the closure crawl, the SBOL2 -> SBOL3 upgrade, and the
//! sequence serializer end to end.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use sbol_db_app::AppServices;
use sbol_db_backend::Backend;
use sbol_db_core::SerializationFormat;
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use sbol_db_storage::GraphWriteMode;
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;

/// The public graph an anonymous caller reads.
const PUBLIC_GRAPH: &str = "http://synbiohub.org/public";

/// The residues seeded on the Sequence; both `/sbol` and `/fasta` must surface
/// them.
const ELEMENTS: &str = "atgcaaatttcccgggtttaaaccc";

/// A public ComponentDefinition at `.../testcoll/testcd/1` referencing a
/// Sequence at `.../testcoll/testseq/1`, expressed in SBOL2 with the
/// `sbh:topLevel` self-marker classic stamps on every top-level object.
const CORPUS: &str = concat!(
    "<http://synbiohub.org/public/testcoll/testcd/1> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://sbols.org/v2#ComponentDefinition> .\n",
    "<http://synbiohub.org/public/testcoll/testcd/1> <http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel> <http://synbiohub.org/public/testcoll/testcd/1> .\n",
    "<http://synbiohub.org/public/testcoll/testcd/1> <http://sbols.org/v2#persistentIdentity> <http://synbiohub.org/public/testcoll/testcd> .\n",
    "<http://synbiohub.org/public/testcoll/testcd/1> <http://sbols.org/v2#displayId> \"testcd\" .\n",
    "<http://synbiohub.org/public/testcoll/testcd/1> <http://sbols.org/v2#version> \"1\" .\n",
    "<http://synbiohub.org/public/testcoll/testcd/1> <http://sbols.org/v2#type> <http://www.biopax.org/release/biopax-level3.owl#DnaRegion> .\n",
    "<http://synbiohub.org/public/testcoll/testcd/1> <http://purl.org/dc/terms/title> \"Test CD\" .\n",
    "<http://synbiohub.org/public/testcoll/testcd/1> <http://sbols.org/v2#sequence> <http://synbiohub.org/public/testcoll/testseq/1> .\n",
    "<http://synbiohub.org/public/testcoll/testseq/1> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://sbols.org/v2#Sequence> .\n",
    "<http://synbiohub.org/public/testcoll/testseq/1> <http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel> <http://synbiohub.org/public/testcoll/testseq/1> .\n",
    "<http://synbiohub.org/public/testcoll/testseq/1> <http://sbols.org/v2#persistentIdentity> <http://synbiohub.org/public/testcoll/testseq> .\n",
    "<http://synbiohub.org/public/testcoll/testseq/1> <http://sbols.org/v2#displayId> \"testseq\" .\n",
    "<http://synbiohub.org/public/testcoll/testseq/1> <http://sbols.org/v2#version> \"1\" .\n",
    "<http://synbiohub.org/public/testcoll/testseq/1> <http://sbols.org/v2#encoding> <http://www.chem.qmul.ac.uk/iubmb/misc/naseq.html> .\n",
    "<http://synbiohub.org/public/testcoll/testseq/1> <http://sbols.org/v2#elements> \"atgcaaatttcccgggtttaaaccc\" .\n",
);

/// A SQLite-backed router over a fresh corpus. The returned `TempDir` owns the
/// database file and must outlive the router.
async fn app_with_corpus() -> (axum::Router, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("download.db");
    let url = format!("sqlite://{}", path.display());
    let backend = Backend::open(&url).await.expect("open sqlite backend");
    backend
        .migrator
        .as_ref()
        .expect("sqlite backend has a migrator")
        .run_migrations()
        .await
        .expect("run migrations");

    backend
        .store
        .graph_store_write(
            PUBLIC_GRAPH,
            CORPUS,
            SerializationFormat::NTriples,
            GraphWriteMode::Merge,
        )
        .await
        .expect("seed corpus");

    let sparql = Arc::new(SparqlEngine::new(backend.triple_source.clone()));
    let sparql_update = Arc::new(SparqlUpdateEngine::new(
        backend.triple_source.clone(),
        backend.triple_writer.clone(),
    ));
    let config = ServerConfig::default();
    let state = AppState {
        service: backend.store.clone(),
        sparql,
        sparql_update,
        app: Arc::new(AppServices::from_backend(&backend)),
        metrics: Metrics::install(None, env!("CARGO_PKG_VERSION")),
        jobs: backend.jobs.clone(),
        lab: backend.lab.clone(),
        config: config.clone(),
        backend_kind: backend.kind,
        sql_console: backend.sql_console.clone(),
        db_stats: backend.db_stats.clone(),
        lsm_stats: backend.lsm_stats.clone(),
        schema_cache: Arc::new(SchemaCache::new()),
    };
    (router(state, config), dir)
}

async fn get(app: &axum::Router, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request")
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = to_bytes(res.into_body(), BODY_LIMIT).await.expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

#[tokio::test]
async fn sbol_returns_the_object_closure() {
    let (app, _dir) = app_with_corpus().await;
    let res = get(&app, "/public/testcoll/testcd/1/sbol").await;
    assert_eq!(res.status(), StatusCode::OK, "/sbol should answer 200");

    let content_type = res
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert_eq!(content_type, "application/rdf+xml");

    let body = body_string(res).await;
    assert!(
        body.contains("http://sbols.org/v2#ComponentDefinition"),
        "the V1 download default must remain classic SBOL2: {body}"
    );
    assert!(
        !body.contains("http://sbols.org/v3#Component"),
        "the V1 default must not silently switch legacy clients to SBOL3: {body}"
    );
    // The RDF closure carries both the object and the referenced sequence it
    // reaches transitively, so the residues appear in the serialization.
    assert!(
        body.contains("testcd"),
        "the SBOL closure should carry the object display id: {body}"
    );
    assert!(
        body.contains(ELEMENTS),
        "the recursive SBOL closure should include the referenced sequence: {body}"
    );
}

#[tokio::test]
async fn sbol_allows_explicit_native_sbol3() {
    let (app, _dir) = app_with_corpus().await;
    let res = get(&app, "/public/testcoll/testcd/1/sbol?version=sbol3").await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "explicit SBOL3 should answer 200"
    );

    let body = body_string(res).await;
    assert!(
        body.contains("http://sbols.org/v3#Component"),
        "an explicit SBOL3 request should return the native view: {body}"
    );
}

#[tokio::test]
async fn fasta_returns_the_sequence_residues() {
    let (app, _dir) = app_with_corpus().await;
    let res = get(&app, "/public/testcoll/testcd/1/fasta").await;
    assert_eq!(res.status(), StatusCode::OK, "/fasta should answer 200");

    let content_type = res
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert_eq!(content_type, "chemical/x-fasta");

    let body = body_string(res).await;
    assert!(
        body.starts_with('>'),
        "FASTA output must open with a record header: {body}"
    );
    // The residues survive the closure crawl, the SBOL2 -> SBOL3 upgrade, and
    // the FASTA serializer.
    assert!(
        body.to_ascii_lowercase().contains(ELEMENTS),
        "FASTA output should carry the seeded residues: {body}"
    );
}

#[tokio::test]
async fn missing_object_is_not_found() {
    let (app, _dir) = app_with_corpus().await;
    let res = get(&app, "/public/testcoll/absent/1/sbol").await;
    let status = res.status();
    let body = body_string(res).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an object with no triples in scope should 404; body: {body}"
    );
}
