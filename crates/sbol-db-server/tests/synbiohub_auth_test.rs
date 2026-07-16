//! HTTP-level integration tests for the SynBioHub V1 auth adapter: the
//! `X-authorization` token flow and the Accept-negotiated `/login`, `/register`,
//! and `/profile` routes. These drive the real axum router via `oneshot` over a
//! SQLite-backed [`AppState`], exercising the same wire shapes classic
//! SynBioHub's clients (and pySBOL2's `PartShop`) send.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use sbol_db_app::AppServices;
use sbol_db_backend::Backend;
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use tempfile::TempDir;
use tower::ServiceExt;
use uuid::Uuid;

const BODY_LIMIT: usize = 4 * 1024 * 1024;

/// Build a router over a fresh SQLite backend with `config`. The returned
/// `TempDir` owns the database file and must outlive the router.
async fn app_with(config: ServerConfig) -> (axum::Router, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("auth.db");
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

/// A router with default config (public signup enabled).
async fn app() -> (axum::Router, TempDir) {
    app_with(ServerConfig::default()).await
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = to_bytes(res.into_body(), BODY_LIMIT).await.expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

fn form(pairs: &[(&str, &str)]) -> String {
    serde_urlencoded::to_string(pairs).expect("encode form")
}

/// Register an account through `POST /register` with an API-style Accept, so the
/// route returns the machine-readable confirmation rather than a redirect.
async fn register(app: &axum::Router, pairs: &[(&str, &str)]) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("accept", "application/json")
                .body(Body::from(form(pairs)))
                .unwrap(),
        )
        .await
        .expect("register request")
}

/// Log in through `POST /login` with an API-style Accept and return the bare
/// plaintext token body, asserting a `200`.
async fn login_token(app: &axum::Router, email: &str, password: &str) -> String {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("accept", "application/json")
                .body(Body::from(form(&[
                    ("email", email),
                    ("password", password),
                ])))
                .unwrap(),
        )
        .await
        .expect("login request");
    assert_eq!(res.status(), StatusCode::OK, "login should succeed");
    body_string(res).await
}

/// `GET /profile` carrying `X-authorization: <token>`.
async fn profile(app: &axum::Router, token: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/profile")
                .header("x-authorization", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("profile request")
}

#[tokio::test]
async fn login_plaintext_token_then_authorizes_reads() {
    let (app, _dir) = app().await;
    let res = register(
        &app,
        &[
            ("name", "Alice Example"),
            ("username", "alice"),
            ("email", "alice@example.org"),
            ("password", "s3cret"),
        ],
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK, "registration should succeed");

    // An API client (no text/html) gets the bare token as the body.
    let token = login_token(&app, "alice@example.org", "s3cret").await;
    assert!(
        Uuid::parse_str(token.trim()).is_ok(),
        "login body should be a bare UUID token, got: {token:?}"
    );

    // The token authorizes a read: the middleware resolves it to the account.
    let res = profile(&app, token.trim()).await;
    assert_eq!(res.status(), StatusCode::OK, "token should authorize reads");
    let body: serde_json::Value = serde_json::from_str(&body_string(res).await).expect("json");
    assert_eq!(body["username"], "alice");
    assert_eq!(body["email"], "alice@example.org");
}

#[tokio::test]
async fn register_403_when_signup_disabled() {
    let config = ServerConfig {
        allow_public_signup: false,
        ..ServerConfig::default()
    };
    let (app, _dir) = app_with(config).await;
    let res = register(
        &app,
        &[
            ("name", "Mallory"),
            ("username", "mallory"),
            ("email", "mallory@example.org"),
            ("password", "s3cret"),
        ],
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::FORBIDDEN,
        "register must be forbidden when public signup is disabled"
    );
}

#[tokio::test]
async fn profile_get_matches_created_fields() {
    let (app, _dir) = app().await;
    let res = register(
        &app,
        &[
            ("name", "Bob Builder"),
            ("username", "bob"),
            ("email", "bob@example.org"),
            ("affiliation", "Acme Labs"),
            ("password", "hunter2"),
        ],
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    let token = login_token(&app, "bob", "hunter2").await;
    let res = profile(&app, token.trim()).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_string(res).await).expect("json");
    assert_eq!(body["name"], "Bob Builder");
    assert_eq!(body["username"], "bob");
    assert_eq!(body["email"], "bob@example.org");
    assert_eq!(body["affiliation"], "Acme Labs");
}

#[tokio::test]
async fn bad_or_missing_token_is_anonymous() {
    let (app, _dir) = app().await;

    // No X-authorization header: the middleware attaches an anonymous
    // CurrentUser and the route answers 401, not a hard 500.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/profile")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("anonymous profile request");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // A bogus token resolves to anonymous just the same.
    let res = profile(&app, "not-a-real-token").await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_ne!(
        res.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "an unknown token must not error out"
    );
}
