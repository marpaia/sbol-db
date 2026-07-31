//! HTTP-level coverage for the opt-in root portal dispatcher.
//!
//! These tests exercise the completed Axum router, not just the classifier.
//! That matters for paths such as `GET /login` (a V1 POST route that would
//! otherwise be 405) and `GET /profile` (an existing V1 GET route): portal
//! navigation must be seen before Axum commits to either result.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::header::{ACCEPT, CONTENT_TYPE, VARY};
use axum::http::{Method, Request, StatusCode};
use sbol_db_app::AppServices;
use sbol_db_backend::Backend;
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;

async fn app_with(config: ServerConfig) -> (axum::Router, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("portal.db");
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

async fn request(
    app: &axum::Router,
    method: Method,
    path: &str,
    accept: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(accept) = accept {
        builder = builder.header(ACCEPT, accept);
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .expect("request")
}

fn is_html(response: &axum::response::Response) -> bool {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"))
}

fn assert_spa(response: &axum::response::Response, path: &str) {
    let expected = if sbol_db_ui::is_built() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    assert_eq!(response.status(), expected, "portal status for {path}");
    assert!(is_html(response), "portal content type for {path}");
    let vary = response
        .headers()
        .get_all(VARY)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>()
        .join(",");
    assert!(
        vary.split(',').any(|value| value.trim() == "Accept"),
        "portal response must vary on Accept for {path}: {vary}"
    );
}

#[tokio::test]
async fn portal_can_be_disabled_without_changing_machine_routes() {
    let config = ServerConfig {
        portal_enabled: false,
        ..ServerConfig::default()
    };
    let (app, _dir) = app_with(config).await;
    let response = request(&app, Method::GET, "/", Some("text/html")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(!is_html(&response));

    // Public page dispatch is off, but the existing admin application and its
    // sign-in entry remain operable (including `/lab` bookmark redirects).
    let response = request(&app, Method::GET, "/admin/graphs", Some("text/html")).await;
    assert_spa(&response, "/admin/graphs");
    let response = request(&app, Method::GET, "/login", Some("text/html")).await;
    assert_spa(&response, "/login");
    let response = request(&app, Method::GET, "/lab", Some("text/html")).await;
    let expected = if sbol_db_ui::is_built() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    assert_eq!(response.status(), expected);
    assert!(is_html(&response));

    // Machine search keeps the compatibility handler in either mode.
    let response = request(&app, Method::GET, "/search", Some("application/json")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!is_html(&response));
}

#[tokio::test]
async fn portal_is_root_mounted_by_default() {
    let (app, _dir) = app_with(ServerConfig::default()).await;
    let response = request(&app, Method::GET, "/", Some("text/html")).await;
    assert_spa(&response, "/");
}

#[tokio::test]
async fn html_navigation_intercepts_new_and_colliding_page_routes() {
    let config = ServerConfig {
        portal_enabled: true,
        ..ServerConfig::default()
    };
    let (app, _dir) = app_with(config).await;

    for path in [
        "/",
        "/login",
        "/profile",
        "/search",
        "/search/objectType=ComponentDefinition",
        "/workspace/shared",
        "/admin/theme",
        "/public/example/missing/1",
    ] {
        let response = request(
            &app,
            Method::GET,
            path,
            Some("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
        )
        .await;
        assert_spa(&response, path);
    }
}

#[tokio::test]
async fn machine_requests_and_legacy_subresources_bypass_the_portal() {
    let config = ServerConfig {
        portal_enabled: true,
        ..ServerConfig::default()
    };
    let (app, _dir) = app_with(config).await;

    let response = request(&app, Method::GET, "/search", Some("application/json")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!is_html(&response));

    let response = request(&app, Method::GET, "/search", Some("*/*")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!is_html(&response));

    let response = request(&app, Method::GET, "/api/v2", Some("text/html")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!is_html(&response));

    let response = request(&app, Method::GET, "/healthz", Some("text/html")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!is_html(&response));

    for path in [
        "/public/example/missing/1/full",
        // Same depth as a versioned identity; the static V1 suffix must win.
        "/public/example/missing/sbol",
        "/user/alice/example/missing/uses",
    ] {
        let response = request(&app, Method::GET, path, Some("text/html")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(!is_html(&response), "{path}");
    }

    let response = request(
        &app,
        Method::GET,
        "/public/example/missing/1",
        Some("text/turtle"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(!is_html(&response));
}

#[tokio::test]
async fn mutations_and_missing_assets_never_fall_back_to_the_spa() {
    let config = ServerConfig {
        portal_enabled: true,
        ..ServerConfig::default()
    };
    let (app, _dir) = app_with(config).await;

    let response = request(&app, Method::POST, "/login", Some("text/html")).await;
    assert_ne!(response.status(), StatusCode::OK);
    assert!(!is_html(&response));

    let response = request(&app, Method::GET, "/assets/does-not-exist.js", Some("*/*")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(!is_html(&response));
}

#[tokio::test]
async fn portal_head_requests_have_headers_without_a_body() {
    let config = ServerConfig {
        portal_enabled: true,
        ..ServerConfig::default()
    };
    let (app, _dir) = app_with(config).await;

    let response = request(&app, Method::HEAD, "/search", Some("text/html")).await;
    assert_spa(&response, "/search");
    let body = to_bytes(response.into_body(), BODY_LIMIT)
        .await
        .expect("head body");
    assert!(body.is_empty(), "HEAD portal response must have no body");
}
