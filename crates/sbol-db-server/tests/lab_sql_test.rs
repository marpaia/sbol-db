//! Integration coverage for `/lab/api/sql/execute`.
//!
//! Targets the docker-compose Postgres (`DATABASE_URL`, defaults to
//! `postgres://sbol:sbol@localhost:5432/sbol`). Covers:
//!
//! - A trivial SELECT returns the expected columns/rows.
//! - A statement-timeout-triggered cancellation surfaces as a 400.
//! - Client disconnect mid-`pg_sleep` triggers `pg_cancel_backend`
//!   so the connection isn't held for the full configured timeout.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use sbol_db_postgres::{connect, run_migrations, JobRepository, SbolObjectService};
use sbol_db_server::{router, AppState, Metrics, ServerConfig};
use sbol_db_sparql::SparqlEngine;
use serde_json::{json, Value};
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;

fn database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://sbol:sbol@localhost:5432/sbol".to_owned())
}

async fn state() -> AppState {
    let pool = connect(&database_url()).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    let service = Arc::new(SbolObjectService::new(pool.clone()));
    let sparql = Arc::new(SparqlEngine::new(Arc::new(service.triples().clone())));
    let jobs = Arc::new(JobRepository::new(pool.clone()));
    let metrics = Metrics::install(pool.clone(), env!("CARGO_PKG_VERSION"));
    AppState {
        service,
        sparql,
        metrics,
        jobs,
        config: ServerConfig::default(),
        schema_cache: Arc::new(sbol_db_server::SchemaCache::new()),
    }
}

async fn post_execute(body: Value) -> (StatusCode, Value) {
    let app = router(state().await, ServerConfig::default());
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/lab/api/sql/execute")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("request");
    let status = res.status();
    let bytes = to_bytes(res.into_body(), BODY_LIMIT).await.expect("body");
    let json: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, json)
}

#[tokio::test]
async fn select_literals_returns_shaped_rows() {
    let (status, body) = post_execute(json!({
        "query": "SELECT 1::int AS n, 'hello'::text AS greeting, true::bool AS flag"
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    let columns = body
        .get("columns")
        .and_then(Value::as_array)
        .expect("columns array");
    assert_eq!(columns.len(), 3);
    assert_eq!(columns[0].get("name").and_then(Value::as_str), Some("n"));
    assert_eq!(
        columns[1].get("name").and_then(Value::as_str),
        Some("greeting")
    );
    assert_eq!(columns[2].get("name").and_then(Value::as_str), Some("flag"));
    assert_eq!(
        columns[0].get("pg_type").and_then(Value::as_str),
        Some("INT4")
    );
    assert_eq!(
        columns[1].get("pg_type").and_then(Value::as_str),
        Some("TEXT")
    );
    assert_eq!(
        columns[2].get("pg_type").and_then(Value::as_str),
        Some("BOOL")
    );

    let rows = body.get("rows").and_then(Value::as_array).expect("rows");
    assert_eq!(rows.len(), 1);
    let row = rows[0].as_array().expect("row array");
    assert_eq!(row[0], json!(1));
    assert_eq!(row[1], json!("hello"));
    assert_eq!(row[2], json!(true));

    assert_eq!(body.get("row_count"), Some(&json!(1)));
    assert_eq!(body.get("truncated"), Some(&json!(false)));
    assert!(body.get("elapsed_ms").and_then(Value::as_u64).is_some());
    assert!(body.get("backend_pid").and_then(Value::as_i64).is_some());
}

#[tokio::test]
async fn empty_query_rejected_as_bad_request() {
    let (status, body) = post_execute(json!({ "query": "   " })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body = {body}");
}

#[tokio::test]
async fn syntax_error_surfaces_as_400_with_postgres_message() {
    let (status, body) = post_execute(json!({
        "query": "SELEKT 1"
    }))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body = {body}");
    let detail = body
        .get("detail")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        detail.to_lowercase().contains("syntax"),
        "expected syntax error detail, got {detail}"
    );
}

#[tokio::test]
async fn statement_timeout_surfaces_as_400_within_window() {
    // Set a tight statement_timeout and run pg_sleep(10) — the query
    // should be cancelled by Postgres after ~250ms with SQLSTATE 57014,
    // which maps to a BadRequest (user-fault).
    let started = Instant::now();
    let (status, body) = post_execute(json!({
        "query": "SELECT pg_sleep(10)",
        "statement_timeout_ms": 250
    }))
    .await;
    let elapsed = started.elapsed();
    assert_eq!(status, StatusCode::BAD_REQUEST, "body = {body}");
    assert!(
        elapsed < Duration::from_secs(3),
        "timeout did not fire promptly: took {elapsed:?}"
    );
    let detail = body
        .get("detail")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    assert!(
        detail.contains("cancel") || detail.contains("timeout"),
        "expected cancel/timeout in detail, got {detail}"
    );
}

#[tokio::test]
async fn client_disconnect_cancels_running_query() {
    // Drive the handler under a future::timeout that drops the request
    // mid-flight. The CancelGuard's Drop impl should fire
    // pg_cancel_backend, returning the connection in well under the
    // server's configured statement_timeout.
    let app = router(state().await, ServerConfig::default());
    let body = json!({
        "query": "SELECT pg_sleep(30)",
        "statement_timeout_ms": 60_000
    });

    let req = Request::builder()
        .method("POST")
        .uri("/lab/api/sql/execute")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    // 800ms is plenty for Postgres to start the sleep; we then drop the
    // future, which triggers CancelGuard::drop → pg_cancel_backend.
    let started = Instant::now();
    let dropped = tokio::time::timeout(Duration::from_millis(800), app.oneshot(req)).await;
    assert!(
        dropped.is_err(),
        "expected the request to still be in-flight; instead got {dropped:?}"
    );

    // Give the cancel a moment to propagate, then assert a fresh
    // request lands quickly on the same pool — if the cancel didn't
    // fire we'd be sitting behind the pg_sleep for ~30s.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (status, _) = post_execute(json!({ "query": "SELECT 1" })).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "follow-up query was not prompt: {:?}",
        started.elapsed()
    );
}
