//! Integration coverage for the operational endpoints: /healthz,
//! /readyz, /metrics. The full route surface is exercised through the
//! postgres crate's tests; this file is scoped to the probes a Helm
//! chart wires up.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use sbol_db_postgres::{connect, run_migrations, JobRepository, SbolObjectService};
use sbol_db_server::{router, AppState, Metrics, ServerConfig};
use sbol_db_sparql::SparqlEngine;
use tower::ServiceExt;

const BODY_LIMIT: usize = 1024 * 1024;

async fn state() -> AppState {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://sbol:sbol@localhost:5432/sbol".to_owned());
    let pool = connect(&database_url).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    let service = Arc::new(SbolObjectService::new(pool.clone()));
    let sparql = Arc::new(SparqlEngine::new(Arc::new(service.quads().clone())));
    let jobs = Arc::new(JobRepository::new(pool.clone()));
    let metrics = Metrics::install(pool.clone(), env!("CARGO_PKG_VERSION"));
    // Wire the worker pool + jobs repo so the /metrics test sees the
    // scrape-time gauges. In a serve setup these come from
    // `build_worker_setup`; for the test we reuse the same pool.
    let metrics = metrics.with_worker_pool(pool).with_jobs_repo(jobs.clone());
    AppState {
        service,
        sparql,
        metrics,
        jobs,
        config: ServerConfig::default(),
        schema_cache: std::sync::Arc::new(sbol_db_server::SchemaCache::new()),
    }
}

async fn read_body(res: axum::response::Response) -> String {
    let bytes = to_bytes(res.into_body(), BODY_LIMIT).await.expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

#[tokio::test]
async fn healthz_returns_ok_literal() {
    let app = router(state().await, sbol_db_server::ServerConfig::default());
    let res = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(read_body(res).await, "ok");
}

#[tokio::test]
async fn readyz_reports_ready_when_db_reachable() {
    let app = router(state().await, sbol_db_server::ServerConfig::default());
    let res = app
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let body = read_body(res).await;
    assert!(
        body.contains("\"status\":\"ready\""),
        "readyz body = {body}"
    );
}

#[tokio::test]
async fn oversize_body_returns_413() {
    let cfg = ServerConfig {
        max_body_bytes: 1024,
        ..ServerConfig::default()
    };
    let app = router(state().await, cfg);
    let body = vec![b'x'; 4096];
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/documents")
                .header("content-type", "text/turtle")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn metrics_exposes_prometheus_format() {
    let app = router(state().await, sbol_db_server::ServerConfig::default());

    // Drive a request through the middleware so the http counter has a
    // sample to render.
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request");

    let res = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let ct = res
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default();
    assert!(ct.starts_with("text/plain"), "content-type = {ct}");
    let body = read_body(res).await;
    for needle in [
        "http_requests_total",
        "http_request_duration_seconds",
        "sbol_db_pool_connections",
        "sbol_db_worker_pool_connections",
        "sbol_db_build_info",
        // Scrape-time worker / job gauges.
        "sbol_db_jobs_status_enum",
    ] {
        assert!(body.contains(needle), "missing {needle} in /metrics output");
    }
    // Templated route label, not the raw path.
    assert!(
        body.contains("route=\"/healthz\""),
        "expected templated route label, got body = {body}"
    );
}
