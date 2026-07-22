//! HTTP-level integration test for the SynBioHub V1 `/search` family.
//!
//! A faceted (`objectType=…`) query carries no free text, so the adapter
//! answers it through the accelerated SPARQL object list rather than the
//! tantivy relevance path. This test drives the real axum router over a
//! SQLite-backed [`AppState`] and asserts the member set the `/search` and
//! `/searchCount` endpoints return matches an independent SPARQL query over the
//! same corpus, proving the faceted path and the engine agree.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use sbol_db_app::AppServices;
use sbol_db_backend::Backend;
use sbol_db_core::SerializationFormat;
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{GraphScope, ResultFormat, SparqlEngine, SparqlOptions, SparqlUpdateEngine};
use sbol_db_storage::GraphWriteMode;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;

/// The public graph an anonymous caller reads.
const PUBLIC_GRAPH: &str = "http://synbiohub.org/public";

const COMPONENT_TYPE: &str = "http://sbols.org/v2#ComponentDefinition";
const CD1: &str = "http://synbiohub.org/public/cd1/1";
const CD2: &str = "http://synbiohub.org/public/cd2/1";
const SEQ1: &str = "http://synbiohub.org/public/seq1/1";

/// An SBOL2 corpus with two top-level ComponentDefinitions and one top-level
/// Sequence, each carrying the `sbh:topLevel` self-marker the faceted template
/// requires.
const CORPUS: &str = concat!(
    "<http://synbiohub.org/public/cd1/1> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://sbols.org/v2#ComponentDefinition> .\n",
    "<http://synbiohub.org/public/cd1/1> <http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel> <http://synbiohub.org/public/cd1/1> .\n",
    "<http://synbiohub.org/public/cd1/1> <http://sbols.org/v2#displayId> \"cd1\" .\n",
    "<http://synbiohub.org/public/cd2/1> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://sbols.org/v2#ComponentDefinition> .\n",
    "<http://synbiohub.org/public/cd2/1> <http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel> <http://synbiohub.org/public/cd2/1> .\n",
    "<http://synbiohub.org/public/cd2/1> <http://sbols.org/v2#displayId> \"cd2\" .\n",
    "<http://synbiohub.org/public/seq1/1> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://sbols.org/v2#Sequence> .\n",
    "<http://synbiohub.org/public/seq1/1> <http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel> <http://synbiohub.org/public/seq1/1> .\n",
    "<http://synbiohub.org/public/seq1/1> <http://sbols.org/v2#displayId> \"seq1\" .\n",
);

/// A SQLite-backed router plus the facade behind it, over a fresh corpus. The
/// returned `TempDir` owns the database file and must outlive the router.
async fn app_with_corpus() -> (axum::Router, Arc<AppServices>, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("search.db");
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
    let app = Arc::new(AppServices::from_backend(&backend));
    let config = ServerConfig::default();
    let state = AppState {
        service: backend.store.clone(),
        sparql,
        sparql_update,
        app: app.clone(),
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
    (router(state, config), app, dir)
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = to_bytes(res.into_body(), BODY_LIMIT).await.expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

/// GET a path and assert a `200`, returning the body.
async fn get_ok(app: &axum::Router, uri: &str) -> String {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK, "{uri} should return 200");
    body_string(res).await
}

/// The `?subject` IRIs bound in a SPARQL-results JSON body.
fn subjects(results_json: &str) -> BTreeSet<String> {
    let value: Value = serde_json::from_str(results_json).expect("parse SPARQL-results JSON");
    value["results"]["bindings"]
        .as_array()
        .expect("bindings array")
        .iter()
        .filter_map(|binding| binding["subject"]["value"].as_str().map(str::to_owned))
        .collect()
}

/// The object URIs in classic's `/search` JSON array (each row's `uri` field).
fn search_uris(search_json: &str) -> BTreeSet<String> {
    let value: Value = serde_json::from_str(search_json).expect("parse search JSON array");
    value
        .as_array()
        .expect("search results array")
        .iter()
        .filter_map(|row| row["uri"].as_str().map(str::to_owned))
        .collect()
}

/// The bare integer classic's `/searchCount` serves in a `text/plain` body.
fn count(count_body: &str) -> u64 {
    count_body.trim().parse().expect("count is an integer")
}

#[tokio::test]
async fn faceted_search_member_set_matches_sparql() {
    let (app, services, _dir) = app_with_corpus().await;

    // The faceted `/search` returns the two ComponentDefinitions, not the
    // Sequence.
    // The trailing `&` marks a facet-only query (empty free text) in the
    // classic grammar, so this goes through the SPARQL path rather than the
    // ranked index.
    let response = get_ok(&app, "/search/objectType=ComponentDefinition&").await;
    let via_search = search_uris(&response);

    // An independent SPARQL query over the same corpus yields the expected set:
    // top-level objects of that class.
    let options = SparqlOptions {
        authorized_graphs: GraphScope::Union,
        ..SparqlOptions::default()
    };
    let outcome = services
        .sparql
        .execute(
            &format!(
                "SELECT DISTINCT ?subject WHERE {{ \
                 ?subject a <{COMPONENT_TYPE}> . \
                 ?subject <http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel> ?subject . }}"
            ),
            Some(ResultFormat::Json),
            None,
            &options,
        )
        .await
        .expect("direct SPARQL");
    let via_sparql = subjects(&String::from_utf8(outcome.payload.body).expect("utf8"));

    assert_eq!(
        via_search, via_sparql,
        "the faceted /search member set matches the direct SPARQL query"
    );
    assert_eq!(
        via_search,
        BTreeSet::from([CD1.to_owned(), CD2.to_owned()]),
        "exactly the two ComponentDefinitions, never the Sequence"
    );
    assert!(
        !via_search.contains(SEQ1),
        "the Sequence is not a ComponentDefinition member"
    );

    // /searchCount reports the same cardinality.
    let count_response = get_ok(&app, "/searchCount/objectType=ComponentDefinition&").await;
    assert_eq!(
        count(&count_response),
        via_sparql.len() as u64,
        "/searchCount matches the member-set size"
    );
}
