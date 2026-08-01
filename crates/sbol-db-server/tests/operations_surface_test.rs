//! Production listener-boundary coverage over a backend-neutral SQLite state.

use std::sync::Arc;

use axum::body::Body;
use axum::http::header::{ACCESS_CONTROL_ALLOW_ORIGIN, ORIGIN};
use axum::http::{HeaderValue, Request, StatusCode};
use axum::Router;
use sbol_db_app::AppServices;
use sbol_db_backend::Backend;
use sbol_db_server::{
    operations_router, public_router, AppState, CorsPolicy, Metrics, SchemaCache, ServerConfig,
};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use tempfile::TempDir;
use tower::ServiceExt;

async fn routers(config: ServerConfig) -> (Router, Router, TempDir) {
    let directory = tempfile::tempdir().expect("tempdir");
    let url = format!("sqlite://{}", directory.path().join("surface.db").display());
    let backend = Backend::open(&url).await.expect("open sqlite backend");
    backend
        .migrator
        .as_ref()
        .expect("sqlite migrator")
        .run_migrations()
        .await
        .expect("migrate");
    let services = Arc::new(AppServices::from_backend(&backend));
    let state = AppState {
        service: backend.store.clone(),
        sparql: Arc::new(SparqlEngine::new(backend.triple_source.clone())),
        sparql_update: Arc::new(SparqlUpdateEngine::new(
            backend.triple_source.clone(),
            backend.triple_writer.clone(),
        )),
        app: services,
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
    (
        public_router(state.clone(), config.clone()),
        operations_router(state, config),
        directory,
    )
}

async fn get(app: &Router, path: &str) -> axum::response::Response {
    app.clone()
        .oneshot(Request::get(path).body(Body::empty()).expect("request"))
        .await
        .expect("response")
}

#[tokio::test]
async fn public_and_operations_routes_are_disjoint() {
    let (public, operations, _directory) = routers(ServerConfig::default()).await;

    assert_eq!(get(&public, "/api/v2").await.status(), StatusCode::OK);
    assert_eq!(
        get(&public, "/healthz").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&public, "/readyz").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&public, "/metrics").await.status(),
        StatusCode::NOT_FOUND
    );

    assert_eq!(get(&operations, "/healthz").await.status(), StatusCode::OK);
    assert_eq!(get(&operations, "/readyz").await.status(), StatusCode::OK);
    assert_eq!(get(&operations, "/metrics").await.status(), StatusCode::OK);
    assert_eq!(
        get(&operations, "/api/v2").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn same_origin_policy_emits_no_cross_origin_permission() {
    let config = ServerConfig {
        cors: CorsPolicy::SameOrigin,
        ..ServerConfig::default()
    };
    let (public, _operations, _directory) = routers(config).await;
    let response = public
        .oneshot(
            Request::get("/api/v2")
                .header(ORIGIN, "https://attacker.example")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert!(response
        .headers()
        .get(ACCESS_CONTROL_ALLOW_ORIGIN)
        .is_none());
}

#[tokio::test]
async fn explicit_cors_allowlist_is_exact() {
    let allowed = HeaderValue::from_static("https://portal.example");
    let config = ServerConfig {
        cors: CorsPolicy::AllowList(vec![allowed.clone()]),
        ..ServerConfig::default()
    };
    let (public, _operations, _directory) = routers(config).await;

    let accepted = public
        .clone()
        .oneshot(
            Request::get("/api/v2")
                .header(ORIGIN, allowed.clone())
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(
        accepted.headers().get(ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&allowed)
    );

    let rejected = public
        .oneshot(
            Request::get("/api/v2")
                .header(ORIGIN, "https://other.example")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert!(rejected
        .headers()
        .get(ACCESS_CONTROL_ALLOW_ORIGIN)
        .is_none());
}

#[tokio::test]
async fn legacy_sparql_write_routes_can_be_absent() {
    let config = ServerConfig {
        sparql_write_enabled: false,
        ..ServerConfig::default()
    };
    let (public, _operations, _directory) = routers(config).await;
    assert_eq!(
        get(&public, "/sparql-auth").await.status(),
        StatusCode::NOT_FOUND
    );
}
