//! Wire-level coverage for the optional SBOLExplorer-compatible listener.
//!
//! These tests build the main and compatibility routers from one SQLite-backed
//! [`AppState`], then prove that the compatibility surface enforces the graph
//! dataset SynBioHub sends, persists its legacy config view, exposes control
//! endpoints, and emits a listener-specific request metric.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use sbol_db_app::AppServices;
use sbol_db_backend::Backend;
use sbol_db_search::ranked_text::IndexedPart;
use sbol_db_server::{explorer_router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 1024 * 1024;
const PUBLIC_GRAPH: &str = "http://synbiohub.org/public";
const PRIVATE_GRAPH: &str = "http://synbiohub.org/user/alice";

async fn app() -> (axum::Router, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("explorer.db");
    let database_url = format!("sqlite://{}", path.display());
    let backend = Backend::open(&database_url).await.expect("open backend");
    backend
        .migrator
        .as_ref()
        .expect("sqlite migrator")
        .run_migrations()
        .await
        .expect("migrate");

    let services = Arc::new(AppServices::from_backend(&backend));
    services
        .text_search
        .rebuild([
            indexed(
                "http://synbiohub.org/public/public_promoter/1",
                PUBLIC_GRAPH,
            ),
            indexed(
                "http://synbiohub.org/user/alice/private_promoter/1",
                PRIVATE_GRAPH,
            ),
        ])
        .expect("build ranked index");

    let config = ServerConfig::default();
    let state = AppState {
        service: backend.store.clone(),
        sparql: Arc::new(SparqlEngine::new(backend.triple_source.clone())),
        sparql_update: Arc::new(SparqlUpdateEngine::new(
            backend.triple_source.clone(),
            backend.triple_writer.clone(),
        )),
        app: services,
        metrics: Metrics::install(None, env!("CARGO_PKG_VERSION"))
            .with_jobs_repo(backend.jobs.clone()),
        jobs: backend.jobs.clone(),
        config: config.clone(),
        lab: backend.lab.clone(),
        backend_kind: backend.kind,
        sql_console: backend.sql_console.clone(),
        db_stats: backend.db_stats.clone(),
        lsm_stats: backend.lsm_stats.clone(),
        schema_cache: Arc::new(SchemaCache::new()),
    };
    (explorer_router(state, config), dir)
}

fn indexed(subject: &str, graph: &str) -> IndexedPart {
    IndexedPart {
        subject: subject.to_owned(),
        graph: graph.to_owned(),
        display_id: Some(subject.rsplit('/').nth(1).unwrap().to_owned()),
        name: Some("promoter".to_owned()),
        description: Some("compatibility test promoter".to_owned()),
        version: Some("1".to_owned()),
        type_iris: vec!["http://sbols.org/v2#ComponentDefinition".to_owned()],
        keywords: "promoter".to_owned(),
        pagerank: 1.0,
    }
}

fn text_query(from: &str) -> String {
    format!(
        "SELECT DISTINCT ?subject ?displayId ?version ?name ?description ?type\n\
         {from} WHERE {{\n\
         FILTER (CONTAINS(lcase(?displayId), lcase('promoter')))\n\
         ?subject a ?type .\n\
         }} LIMIT 50 OFFSET 0"
    )
}

fn browse_query(count: bool) -> String {
    let projection = if count {
        "select (sum(?tempcount) as ?count) WHERE { { SELECT (count(distinct ?subject) as ?tempcount)"
    } else {
        "SELECT DISTINCT ?subject ?displayId ?version ?name ?description ?type"
    };
    let close = if count { "} }" } else { "" };
    format!(
        "{projection} WHERE {{\n\
         ?subject a ?type .\n\
         ?subject sbh:topLevel ?subject .\n\
         OPTIONAL {{ ?subject sbol2:displayId ?displayId . }}\n\
         }} {close} LIMIT 50 OFFSET 0"
    )
}

fn search_uri(query: &str, default_graph: Option<&str>) -> String {
    let mut pairs = vec![("query", query)];
    if let Some(default_graph) = default_graph {
        pairs.push(("default-graph-uri", default_graph));
    }
    format!(
        "/?{}",
        serde_urlencoded::to_string(pairs).expect("encode query")
    )
}

async fn request(app: &axum::Router, request: Request<Body>) -> axum::response::Response {
    app.clone().oneshot(request).await.expect("request")
}

async fn body(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), BODY_LIMIT)
        .await
        .expect("response body");
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

fn subjects(response: &str) -> Vec<String> {
    let value: Value = serde_json::from_str(response).expect("SPARQL JSON");
    value["results"]["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .map(|binding| binding["subject"]["value"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn root_listener_enforces_default_and_from_graphs() {
    let (app, _dir) = app().await;

    let public = request(
        &app,
        Request::get(search_uri(&text_query(""), Some(PUBLIC_GRAPH)))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(public.status(), StatusCode::OK);
    assert_eq!(
        subjects(&body(public).await),
        vec!["http://synbiohub.org/public/public_promoter/1"]
    );

    let both_query = text_query(&format!("FROM <{PUBLIC_GRAPH}> FROM <{PRIVATE_GRAPH}>"));
    let both = request(
        &app,
        Request::get(search_uri(&both_query, Some("http://ignored/default")))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(both.status(), StatusCode::OK);
    let mut both_subjects = subjects(&body(both).await);
    both_subjects.sort();
    assert_eq!(
        both_subjects,
        vec![
            "http://synbiohub.org/public/public_promoter/1",
            "http://synbiohub.org/user/alice/private_promoter/1",
        ]
    );

    let no_dataset = request(
        &app,
        Request::get(search_uri(&text_query(""), None))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(no_dataset.status(), StatusCode::OK);
    assert!(subjects(&body(no_dataset).await).is_empty());
}

#[tokio::test]
async fn empty_synbiohub_search_is_a_scoped_ranked_browse() {
    let (app, _dir) = app().await;

    let rows = request(
        &app,
        Request::get(search_uri(&browse_query(false), Some(PUBLIC_GRAPH)))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(rows.status(), StatusCode::OK);
    assert_eq!(
        subjects(&body(rows).await),
        vec!["http://synbiohub.org/public/public_promoter/1"]
    );

    let count = request(
        &app,
        Request::get(search_uri(&browse_query(true), Some(PUBLIC_GRAPH)))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(count.status(), StatusCode::OK);
    let count: Value = serde_json::from_str(&body(count).await).unwrap();
    assert_eq!(count["results"]["bindings"][0]["count"]["value"], "1");
}

#[tokio::test]
async fn config_round_trips_and_control_endpoints_are_compatible() {
    let (app, _dir) = app().await;

    let initial = request(&app, Request::get("/config").body(Body::empty()).unwrap()).await;
    assert_eq!(initial.status(), StatusCode::OK);
    let initial: Value = serde_json::from_str(&body(initial).await).unwrap();
    assert_eq!(initial["engine"], "sbol-db");
    assert_eq!(initial["uclust_identity"], "0.8");

    let updated = request(
        &app,
        Request::post("/config")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"uclust_identity":"0.9","custom":"kept"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let updated: Value = serde_json::from_str(&body(updated).await).unwrap();
    assert_eq!(updated["uclust_identity"], "0.9");
    assert_eq!(updated["custom"], "kept");
    assert_eq!(updated["engine"], "sbol-db");

    for path in [
        "/info",
        "/indexinginfo",
        "/healthz",
        "/readyz",
        "/incrementalremove?subject=http%3A%2F%2Fexample.org%2Fx",
        "/incrementalremovecollection?subject=http%3A%2F%2Fexample.org%2Fx",
    ] {
        let response = request(&app, Request::get(path).body(Body::empty()).unwrap()).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }

    let incremental = request(
        &app,
        Request::post("/incrementalupdate")
            .header("content-type", "application/json")
            .body(Body::from("[]"))
            .unwrap(),
    )
    .await;
    assert_eq!(incremental.status(), StatusCode::OK);

    let rebuild = request(&app, Request::get("/update").body(Body::empty()).unwrap()).await;
    assert_eq!(rebuild.status(), StatusCode::ACCEPTED);
    assert!(rebuild.headers().contains_key("x-sbol-db-job-id"));

    let indexing = request(
        &app,
        Request::get("/indexinginfo").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(indexing.status(), StatusCode::OK);
    let indexing = body(indexing).await;
    assert!(indexing.contains("job_id="), "{indexing}");
    assert!(indexing.contains("status=queued"), "{indexing}");
}

#[tokio::test]
async fn metrics_prove_the_compatibility_listener_received_requests() {
    let (app, _dir) = app().await;
    let _ = request(&app, Request::get("/info").body(Body::empty()).unwrap()).await;
    let metrics = request(&app, Request::get("/metrics").body(Body::empty()).unwrap()).await;
    assert_eq!(metrics.status(), StatusCode::OK);
    let metrics = body(metrics).await;
    assert!(
        metrics.contains("sbol_db_explorer_requests_total"),
        "listener-specific counter absent: {metrics}"
    );
    assert!(metrics.contains("route=\"/info\""), "route label absent");
}
