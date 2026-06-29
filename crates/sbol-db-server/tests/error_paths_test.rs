//! Error-path coverage for the HTTP routes. Happy paths are covered by
//! `health_test.rs` and the postgres crate's integration tests; this file
//! pins the 4xx responses that protect against malformed input. Each test
//! exercises one route family.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use sbol_db_postgres::{connect, run_migrations, JobRepository, SbolObjectService};
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use tower::ServiceExt;

const BODY_LIMIT: usize = 1024 * 1024;

async fn state() -> AppState {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://sbol:sbol@localhost:5432/sbol".to_owned());
    let pool = connect(&database_url).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    let service = Arc::new(SbolObjectService::new(pool.clone()));
    let sparql = Arc::new(SparqlEngine::new(service.triple_source()));
    let sparql_update = Arc::new(SparqlUpdateEngine::new(
        service.triple_source(),
        service.triple_writer(),
    ));
    let jobs = Arc::new(JobRepository::new(pool.clone()));
    let pg_pool = Some(pool.clone());
    let metrics = Metrics::install(Some(pool.clone()), env!("CARGO_PKG_VERSION"))
        .with_worker_pool(pool)
        .with_jobs_repo(jobs.clone());
    AppState {
        lab: service.clone(),
        service,
        sparql,
        sparql_update,
        metrics,
        jobs,
        config: ServerConfig::default(),
        pg_pool,
        schema_cache: Arc::new(SchemaCache::new()),
    }
}

async fn read_body(res: axum::response::Response) -> String {
    let bytes = to_bytes(res.into_body(), BODY_LIMIT).await.expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

async fn send(req: Request<Body>) -> axum::response::Response {
    let app = router(state().await, ServerConfig::default());
    app.oneshot(req).await.expect("request")
}

// ---------------------------------------------------------------------------
// /graphs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_document_without_format_or_content_type_is_400() {
    let res = send(
        Request::builder()
            .method("POST")
            .uri("/graphs")
            .body(Body::from("not really turtle"))
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = read_body(res).await;
    assert!(
        body.contains("format"),
        "body should reference format: {body}"
    );
}

#[tokio::test]
async fn create_document_with_unknown_format_is_400() {
    let res = send(
        Request::builder()
            .method("POST")
            .uri("/graphs?format=parquet")
            .body(Body::from(""))
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = read_body(res).await;
    assert!(body.contains("unknown format"), "got {body}");
}

#[tokio::test]
async fn create_document_with_invalid_iri_is_400() {
    let res = send(
        Request::builder()
            .method("POST")
            .uri("/graphs?format=turtle&document_iri=not-an-iri")
            .header("content-type", "text/turtle")
            .body(Body::from("# empty"))
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// /sparql
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sparql_update_query_is_rejected_400() {
    let update = "INSERT DATA { <https://x> <https://p> <https://o> }";
    let res = send(
        Request::builder()
            .method("POST")
            .uri("/sparql")
            .header("content-type", "application/sparql-query")
            .body(Body::from(update))
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = read_body(res).await;
    assert!(
        body.contains("sparql_update_not_allowed") || body.to_lowercase().contains("update"),
        "expected sparql_update_not_allowed error code, got {body}"
    );
}

#[tokio::test]
async fn sparql_get_without_query_param_is_400() {
    let res = send(
        Request::builder()
            .uri("/sparql")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sparql_malformed_query_is_400() {
    let res = send(
        Request::builder()
            .method("POST")
            .uri("/sparql")
            .header("content-type", "application/sparql-query")
            .body(Body::from("SELECT ?s WHERE { ?s ?p"))
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// /objects/neighborhood
// ---------------------------------------------------------------------------

#[tokio::test]
async fn neighborhood_with_invalid_iri_is_400() {
    let res = send(
        Request::builder()
            .uri("/objects/neighborhood?iri=not-an-iri")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn neighborhood_with_unknown_direction_is_400() {
    let res = send(
        Request::builder()
            .uri("/objects/neighborhood?iri=https://example.org/x&direction=sideways")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = read_body(res).await;
    assert!(body.contains("direction"), "got {body}");
}

#[tokio::test]
async fn neighborhood_rdf_with_unknown_format_is_400() {
    let res = send(
        Request::builder()
            .uri("/objects/neighborhood.rdf?iri=https://example.org/x&format=yaml")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = read_body(res).await;
    assert!(body.contains("unknown format"), "got {body}");
}

// ---------------------------------------------------------------------------
// /objects
// ---------------------------------------------------------------------------

#[tokio::test]
async fn objects_lookup_with_missing_iri_is_400() {
    let res = send(
        Request::builder()
            .uri("/objects")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    // axum's Query extractor surfaces missing required fields as 400.
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn get_document_with_malformed_uuid_is_400_or_404() {
    let res = send(
        Request::builder()
            .uri("/graphs/not-a-uuid")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(
        matches!(
            res.status(),
            StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND
        ),
        "expected 400/404 for malformed UUID, got {}",
        res.status()
    );
}

#[tokio::test]
async fn get_document_with_unknown_uuid_is_404() {
    let res = send(
        Request::builder()
            .uri("/graphs/00000000-0000-0000-0000-000000000000")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// /jobs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_job_with_unknown_uuid_is_404() {
    let res = send(
        Request::builder()
            .uri("/jobs/00000000-0000-0000-0000-000000000000")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// /sequences/search
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sequence_search_without_pattern_is_400() {
    let res = send(
        Request::builder()
            .uri("/sequences/search")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// Catch-all
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_route_returns_404_with_json_body() {
    let res = send(
        Request::builder()
            .uri("/this/route/does/not/exist")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let body = read_body(res).await;
    assert!(body.contains("\"not_found\""), "got {body}");
}
