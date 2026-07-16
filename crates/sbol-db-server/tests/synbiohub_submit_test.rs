//! HTTP-level integration tests for the SynBioHub V1 `POST /submit` adapter.
//!
//! These drive the real axum router via `oneshot` over a SQLite-backed
//! [`AppState`]. An authenticated caller submits an SBOL document, and the
//! minted collection and its members read back through `/manage` at the expected
//! SynBioHub URIs, owned by the caller. An anonymous submit is rejected with
//! `403`, proving the write-authorization gate holds.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use sbol_db_app::AppServices;
use sbol_db_backend::Backend;
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;
const BOUNDARY: &str = "sboldbsubmitboundary";

/// A compliant SBOL2 document (Turtle): a ComponentDefinition with a nested
/// SequenceAnnotation child, plus a standalone Sequence, all versioned `1`.
const FIXTURE: &str = r#"
@prefix sbol: <http://sbols.org/v2#> .
@prefix dcterms: <http://purl.org/dc/terms/> .

<http://example.org/cd/1>
    a sbol:ComponentDefinition ;
    sbol:displayId "cd" ;
    sbol:persistentIdentity <http://example.org/cd> ;
    sbol:version "1" ;
    dcterms:title "My Component" ;
    sbol:sequenceAnnotation <http://example.org/cd/anno/1> .

<http://example.org/cd/anno/1>
    a sbol:SequenceAnnotation ;
    sbol:displayId "anno" ;
    sbol:persistentIdentity <http://example.org/cd/anno> ;
    sbol:version "1" .

<http://example.org/seq/1>
    a sbol:Sequence ;
    sbol:displayId "seq" ;
    sbol:persistentIdentity <http://example.org/seq> ;
    sbol:version "1" ;
    sbol:elements "atgc" .
"#;

/// Build a router over a fresh SQLite backend. The returned `TempDir` owns the
/// database file and must outlive the router.
async fn app() -> (axum::Router, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("submit.db");
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

async fn body_string(res: axum::response::Response) -> String {
    let bytes = to_bytes(res.into_body(), BODY_LIMIT).await.expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

fn form_field(name: &str, value: &str) -> String {
    format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
}

/// A `multipart/form-data` body carrying the submission form fields and the SBOL
/// file part.
fn submit_body() -> String {
    let mut body = String::new();
    body.push_str(&form_field("id", "mysubmission"));
    body.push_str(&form_field("version", "1"));
    body.push_str(&form_field("name", "My Submission"));
    body.push_str(&form_field("description", "A test submission"));
    body.push_str(&form_field("citations", "12345678"));
    body.push_str(&form_field("overwrite_merge", "0"));
    body.push_str(&form_field("format", "turtle"));
    body.push_str(&format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"doc.ttl\"\r\nContent-Type: text/turtle\r\n\r\n{FIXTURE}\r\n"
    ));
    body.push_str(&format!("--{BOUNDARY}--\r\n"));
    body
}

async fn register(app: &axum::Router, pairs: &[(&str, &str)]) {
    let form = serde_urlencoded::to_string(pairs).expect("encode form");
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("accept", "application/json")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .expect("register request");
    assert_eq!(res.status(), StatusCode::OK, "registration should succeed");
}

async fn login_token(app: &axum::Router, email: &str, password: &str) -> String {
    let form = serde_urlencoded::to_string([("email", email), ("password", password)])
        .expect("encode form");
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("accept", "application/json")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .expect("login request");
    assert_eq!(res.status(), StatusCode::OK, "login should succeed");
    body_string(res).await.trim().to_owned()
}

fn submit_request(token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("POST").uri("/submit").header(
        "content-type",
        format!("multipart/form-data; boundary={BOUNDARY}"),
    );
    if let Some(token) = token {
        builder = builder.header("x-authorization", token);
    }
    builder.body(Body::from(submit_body())).unwrap()
}

#[tokio::test]
async fn authenticated_submit_reads_back_minted_collection_and_members() {
    let (app, _dir) = app().await;
    register(
        &app,
        &[
            ("name", "Alice Example"),
            ("username", "alice"),
            ("email", "alice@example.org"),
            ("password", "s3cret"),
        ],
    )
    .await;
    let token = login_token(&app, "alice@example.org", "s3cret").await;

    // Submit as the authenticated caller.
    let res = app
        .clone()
        .oneshot(submit_request(Some(&token)))
        .await
        .expect("submit request");
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "authenticated submit succeeds"
    );
    let outcome: serde_json::Value =
        serde_json::from_str(&body_string(res).await).expect("submit json");

    let collection = "http://synbiohub.org/user/alice/mysubmission/mysubmission_collection/1";
    let component = "http://synbiohub.org/user/alice/mysubmission/cd/1";
    let sequence = "http://synbiohub.org/user/alice/mysubmission/seq/1";

    assert_eq!(
        outcome["collectionUri"], collection,
        "the response reports the minted collection URI"
    );
    let members: Vec<&str> = outcome["members"]
        .as_array()
        .expect("members array")
        .iter()
        .map(|m| m.as_str().expect("member string"))
        .collect();
    assert!(
        members.contains(&component),
        "members include the component"
    );
    assert!(members.contains(&sequence), "members include the sequence");

    // Read the submission back through /manage: the owned-object listing the
    // caller sees under its own scope. The collection and both members appear,
    // owned by the caller, at the minted URIs.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/manage")
                .header("x-authorization", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("manage request");
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "manage is readable by the owner"
    );
    let manage = body_string(res).await;
    assert!(
        manage.contains(collection),
        "manage lists the minted collection: {manage}"
    );
    assert!(
        manage.contains(component),
        "manage lists the minted component: {manage}"
    );
    assert!(
        manage.contains(sequence),
        "manage lists the minted sequence: {manage}"
    );
}

#[tokio::test]
async fn anonymous_submit_is_forbidden() {
    let (app, _dir) = app().await;
    let res = app
        .clone()
        .oneshot(submit_request(None))
        .await
        .expect("anonymous submit request");
    assert_eq!(
        res.status(),
        StatusCode::FORBIDDEN,
        "an anonymous caller may not submit"
    );
}

#[tokio::test]
async fn resubmit_without_overwrite_is_rejected() {
    let (app, _dir) = app().await;
    register(
        &app,
        &[
            ("name", "Alice Example"),
            ("username", "alice"),
            ("email", "alice@example.org"),
            ("password", "s3cret"),
        ],
    )
    .await;
    let token = login_token(&app, "alice@example.org", "s3cret").await;

    let first = app
        .clone()
        .oneshot(submit_request(Some(&token)))
        .await
        .expect("first submit");
    assert_eq!(first.status(), StatusCode::OK, "first submit succeeds");

    // A second submit with overwrite_merge=0 collides with the existing
    // id/version and is rejected.
    let second = app
        .clone()
        .oneshot(submit_request(Some(&token)))
        .await
        .expect("second submit");
    assert_eq!(
        second.status(),
        StatusCode::BAD_REQUEST,
        "resubmitting the same id/version without overwrite is rejected"
    );
}
