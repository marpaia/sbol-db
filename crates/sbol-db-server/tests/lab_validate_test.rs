//! Coverage for `/lab/api/{sql,sparql}/validate`.
//!
//! These tests exercise the parse-only path. They don't touch the
//! database, but the test harness still wires up an `AppState` so
//! the router can build — the actual handlers don't read state.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use sbol_db_postgres::{connect, run_migrations, JobRepository, SbolObjectService};
use sbol_db_server::{router, AppState, Metrics, ServerConfig};
use sbol_db_sparql::SparqlEngine;
use serde_json::{json, Value};
use tower::ServiceExt;

const BODY_LIMIT: usize = 1024 * 1024;

fn database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://sbol:sbol@localhost:5432/sbol".to_owned())
}

async fn state() -> AppState {
    let pool = connect(&database_url()).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    let service = Arc::new(SbolObjectService::new(pool.clone()));
    let sparql = Arc::new(SparqlEngine::new(Arc::new(service.quads().clone())));
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

async fn post(path: &str, body: Value) -> (StatusCode, Value) {
    let app = router(state().await, ServerConfig::default());
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
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

// ---------- SQL ----------

#[tokio::test]
async fn sql_validate_accepts_valid_query() {
    let (status, body) = post(
        "/lab/api/sql/validate",
        json!({ "query": "SELECT 1 FROM sbol_objects WHERE iri = 'x'" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    assert_eq!(body.get("ok"), Some(&json!(true)));
    assert!(body
        .get("errors")
        .and_then(Value::as_array)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn sql_validate_accepts_empty_query_as_ok() {
    // Empty buffers shouldn't show squigglies — the UI debounces on
    // every keystroke, including when the user wipes the input.
    let (status, body) = post("/lab/api/sql/validate", json!({ "query": "" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.get("ok"), Some(&json!(true)));
}

#[tokio::test]
async fn sql_validate_pinpoints_syntax_error_position() {
    let (status, body) = post(
        "/lab/api/sql/validate",
        json!({ "query": "SELEKT 1\nFROM foo" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    assert_eq!(body.get("ok"), Some(&json!(false)));
    let errs = body
        .get("errors")
        .and_then(Value::as_array)
        .expect("errors");
    assert_eq!(errs.len(), 1);
    let line = errs[0].get("line").and_then(Value::as_u64).expect("line");
    let column = errs[0].get("column").and_then(Value::as_u64).expect("col");
    assert_eq!(line, 1, "error should land on the SELEKT line");
    assert!(column >= 1, "column = {column}");
    let msg = errs[0].get("message").and_then(Value::as_str).unwrap_or("");
    assert!(
        msg.to_lowercase().contains("syntax"),
        "expected 'syntax' in message, got {msg}"
    );
}

#[tokio::test]
async fn sql_validate_returns_position_for_multiline_query() {
    // libpg_query's cursor can be conservative for compound errors —
    // it may report the start of the malformed statement rather than
    // the offending token. We assert the response shape is correct
    // (single error, 1-indexed line/column) without prescribing where
    // exactly the parser pinpoints.
    let (status, body) = post(
        "/lab/api/sql/validate",
        json!({ "query": "SELECT 1\nFROM sbol_objects\nWHERR iri = 'x'" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    let errs = body
        .get("errors")
        .and_then(Value::as_array)
        .expect("errors");
    assert_eq!(errs.len(), 1);
    let line = errs[0].get("line").and_then(Value::as_u64).expect("line");
    let column = errs[0].get("column").and_then(Value::as_u64).expect("col");
    assert!(line >= 1 && column >= 1);
}

// ---------- SPARQL ----------

#[tokio::test]
async fn sparql_validate_accepts_valid_query() {
    let (status, body) = post(
        "/lab/api/sparql/validate",
        json!({ "query": "ASK { ?s ?p ?o }" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    assert_eq!(body.get("ok"), Some(&json!(true)));
}

#[tokio::test]
async fn sparql_validate_accepts_empty_query_as_ok() {
    let (status, body) = post("/lab/api/sparql/validate", json!({ "query": "" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.get("ok"), Some(&json!(true)));
}

#[tokio::test]
async fn sparql_validate_pinpoints_syntax_error_position() {
    let (status, body) = post(
        "/lab/api/sparql/validate",
        json!({ "query": "SELECT NOPE ?s WHERE { ?s ?p ?o }" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    assert_eq!(body.get("ok"), Some(&json!(false)));
    let errs = body
        .get("errors")
        .and_then(Value::as_array)
        .expect("errors");
    assert_eq!(errs.len(), 1);
    let line = errs[0].get("line").and_then(Value::as_u64).expect("line");
    let column = errs[0].get("column").and_then(Value::as_u64).expect("col");
    assert_eq!(line, 1);
    assert!(column > 1, "expected error past column 1, got {column}");
}
