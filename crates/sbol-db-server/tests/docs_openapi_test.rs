//! Contract tests for the `/docs` render surface and the OpenAPI documents it
//! embeds. These drive the real axum router over a SQLite backend to prove that
//! the docs page, the V1 spec, and the V2 spec are all served, parse as valid
//! JSON, and document a representative set of both surfaces.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, Response, StatusCode};
use sbol_db_app::AppServices;
use sbol_db_backend::Backend;
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;

/// Build a router over a fresh SQLite backend. The returned `TempDir` owns the
/// database file and must outlive the router.
async fn app() -> (axum::Router, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("docs.db");
    let url = format!("sqlite://{}", path.display());
    let backend = Backend::open(&url).await.expect("open sqlite backend");
    backend
        .migrator
        .as_ref()
        .expect("sqlite backend has a migrator")
        .run_migrations()
        .await
        .expect("run migrations");

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

async fn body_string(res: Response<Body>) -> String {
    let bytes = to_bytes(res.into_body(), BODY_LIMIT).await.expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

/// Issue one GET, returning the status and body.
async fn get(app: &axum::Router, uri: &str) -> (StatusCode, String) {
    let res = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .expect("request");
    let status = res.status();
    (status, body_string(res).await)
}

#[tokio::test]
async fn docs_page_renders_both_surfaces_in_one_reference() {
    let (app, _dir) = app().await;
    let (status, body) = get(&app, "/docs").await;
    assert_eq!(status, StatusCode::OK, "the docs page is served");
    assert!(
        body.contains("Scalar.createApiReference"),
        "the docs page mounts via the explicit createApiReference call that drives multi-source"
    );
    assert!(
        body.contains("/openapi.json"),
        "the docs page includes the V1 source"
    );
    assert!(
        body.contains("/api/v2/openapi.json"),
        "the docs page includes the V2 source in the same reference"
    );
    // The V2 surface also stays directly reachable at its own page.
    let (v2_status, v2_body) = get(&app, "/api/v2/docs").await;
    assert_eq!(v2_status, StatusCode::OK, "the V2 docs page is served");
    assert!(
        v2_body.contains("data-url=\"/api/v2/openapi.json\""),
        "the V2 docs page renders the V2 spec"
    );
}

#[tokio::test]
async fn v1_spec_is_served_and_documents_representative_paths() {
    let (app, _dir) = app().await;
    let (status, body) = get(&app, "/openapi.json").await;
    assert_eq!(status, StatusCode::OK, "the V1 spec is served");
    let spec: Value = serde_json::from_str(&body).expect("V1 spec is valid JSON");
    assert_eq!(spec["openapi"], "3.1.0", "declares OpenAPI 3.1");

    let paths = spec["paths"].as_object().expect("paths object");
    for path in ["/login", "/logout", "/register", "/submit", "/search"] {
        assert!(paths.contains_key(path), "the V1 spec documents {path}");
    }
    // The SBOL download lives under the object route hierarchy.
    assert!(
        paths.keys().any(|p| p.ends_with("/sbol")),
        "the V1 spec documents an SBOL download route"
    );
}

#[tokio::test]
async fn v2_spec_is_served_and_documents_representative_paths() {
    let (app, _dir) = app().await;
    let (status, body) = get(&app, "/api/v2/openapi.json").await;
    assert_eq!(status, StatusCode::OK, "the V2 spec is served");
    let spec: Value = serde_json::from_str(&body).expect("V2 spec is valid JSON");
    assert_eq!(spec["openapi"], "3.1.0", "declares OpenAPI 3.1");
    assert_eq!(spec["servers"][0]["url"], "/api/v2", "based at /api/v2");

    let paths = spec["paths"].as_object().expect("paths object");
    for path in ["/objects", "/search"] {
        assert!(paths.contains_key(path), "the V2 spec documents {path}");
    }
}

#[tokio::test]
async fn both_spec_routes_serve_valid_json() {
    let (app, _dir) = app().await;
    for uri in ["/openapi.json", "/api/v2/openapi.json"] {
        let (status, body) = get(&app, uri).await;
        assert_eq!(status, StatusCode::OK, "{uri} is served");
        serde_json::from_str::<Value>(&body).unwrap_or_else(|e| panic!("{uri} is valid JSON: {e}"));
    }
}
