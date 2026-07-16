//! HTTP-level integration tests for the SynBioHub V1 field-edit and permission
//! routes.
//!
//! These drive the real axum router via `oneshot` over a SQLite-backed
//! [`AppState`]. An owner edits a submitted object's mutable description and
//! title, and the read-back through the unscoped `/sparql` endpoint shows the new
//! values with a freshly bumped `dcterms:modified`; a non-owner and an anonymous
//! caller are both rejected with `403`; and after an owner grants a second user
//! ownership through `/addOwner`, that user sees the object in `/manage`, then
//! loses it again after `/removeOwner`.

use std::sync::Arc;
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use sbol_db_app::AppServices;
use sbol_db_backend::Backend;
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;
const BOUNDARY: &str = "sboldbeditboundary";

const PRIVATE_COLLECTION: &str =
    "http://synbiohub.org/user/alice/mysubmission/mysubmission_collection/1";
const DCTERMS_MODIFIED: &str = "http://purl.org/dc/terms/modified";
const DCTERMS_TITLE: &str = "http://purl.org/dc/terms/title";
const SBH_MUTABLE_DESCRIPTION: &str =
    "http://wiki.synbiohub.org/wiki/Terms/synbiohub#mutableDescription";

/// A compliant SBOL2 document (Turtle): a ComponentDefinition, a nested
/// SequenceAnnotation child, and a standalone Sequence, all versioned `1`.
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

async fn app() -> (axum::Router, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("edit.db");
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

fn submit_body() -> String {
    let mut body = String::new();
    body.push_str(&form_field("id", "mysubmission"));
    body.push_str(&form_field("version", "1"));
    body.push_str(&form_field("name", "My Submission"));
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

async fn register_and_login(app: &axum::Router, name: &str, username: &str) -> String {
    let email = format!("{username}@example.org");
    register(
        app,
        &[
            ("name", name),
            ("username", username),
            ("email", &email),
            ("password", "s3cret"),
        ],
    )
    .await;
    login_token(app, &email, "s3cret").await
}

async fn submit(app: &axum::Router, token: &str) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/submit")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={BOUNDARY}"),
                )
                .header("x-authorization", token)
                .body(Body::from(submit_body()))
                .unwrap(),
        )
        .await
        .expect("submit request");
    assert_eq!(res.status(), StatusCode::OK, "submit should succeed");
}

/// POST a form-encoded body, returning the response status.
async fn post_form(
    app: &axum::Router,
    uri: &str,
    token: Option<&str>,
    pairs: &[(&str, &str)],
) -> (StatusCode, String) {
    let form = serde_urlencoded::to_string(pairs).expect("encode form");
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/x-www-form-urlencoded");
    if let Some(token) = token {
        builder = builder.header("x-authorization", token);
    }
    let res = app
        .clone()
        .oneshot(builder.body(Body::from(form)).unwrap())
        .await
        .expect("post request");
    let status = res.status();
    (status, body_string(res).await)
}

async fn get(app: &axum::Router, uri: &str, token: Option<&str>) -> (StatusCode, String) {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(token) = token {
        builder = builder.header("x-authorization", token);
    }
    let res = app
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .expect("get request");
    let status = res.status();
    (status, body_string(res).await)
}

/// The object values of `<subject> <predicate> ?o`, read through the unscoped
/// `/sparql` endpoint (union of all named graphs, so the private submission is
/// visible).
async fn sparql_objects(app: &axum::Router, subject: &str, predicate: &str) -> Vec<String> {
    let query = format!("SELECT ?o WHERE {{ <{subject}> <{predicate}> ?o }}");
    let qs = serde_urlencoded::to_string([("query", query.as_str())]).expect("encode query");
    let (status, body) = get(app, &format!("/sparql?{qs}"), None).await;
    assert_eq!(status, StatusCode::OK, "sparql read: {body}");
    let json: serde_json::Value = serde_json::from_str(&body).expect("sparql json");
    json["results"]["bindings"]
        .as_array()
        .expect("bindings array")
        .iter()
        .filter_map(|b| b["o"]["value"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn owner_edits_refresh_modified() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "Alice Example", "alice").await;
    submit(&app, &token).await;

    let modified_before = sparql_objects(&app, PRIVATE_COLLECTION, DCTERMS_MODIFIED)
        .await
        .into_iter()
        .next()
        .expect("mint stamps dcterms:modified");

    // Cross a whole-second boundary so the refreshed second-resolution timestamp
    // is strictly greater than the mint's.
    tokio::time::sleep(Duration::from_millis(1_100)).await;

    // Mutable description: uri in the body.
    let (status, _) = post_form(
        &app,
        "/updateMutableDescription",
        Some(&token),
        &[
            ("uri", PRIVATE_COLLECTION),
            ("value", "A mutable description"),
        ],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "owner updateMutableDescription succeeds"
    );

    // Generic field edit: dcterms:title through the path-based /edit/:field.
    let (status, _) = post_form(
        &app,
        "/user/alice/mysubmission/mysubmission_collection/1/edit/title",
        Some(&token),
        &[("object", "Renamed Collection")],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner edit/title succeeds");

    let descriptions = sparql_objects(&app, PRIVATE_COLLECTION, SBH_MUTABLE_DESCRIPTION).await;
    assert_eq!(
        descriptions,
        vec!["A mutable description".to_owned()],
        "the mutable description is set exactly once"
    );

    let titles = sparql_objects(&app, PRIVATE_COLLECTION, DCTERMS_TITLE).await;
    assert_eq!(
        titles,
        vec!["Renamed Collection".to_owned()],
        "the title is replaced, not duplicated"
    );

    let modified_after = sparql_objects(&app, PRIVATE_COLLECTION, DCTERMS_MODIFIED).await;
    assert_eq!(
        modified_after.len(),
        1,
        "dcterms:modified stays singular after edits: {modified_after:?}"
    );
    assert!(
        modified_after[0] > modified_before,
        "dcterms:modified was refreshed: {} -> {}",
        modified_before,
        modified_after[0]
    );
}

#[tokio::test]
async fn non_owner_and_anonymous_edits_forbidden() {
    let (app, _dir) = app().await;
    let alice = register_and_login(&app, "Alice Example", "alice").await;
    submit(&app, &alice).await;

    let bob = register_and_login(&app, "Bob Example", "bob").await;

    // A different authenticated user may not edit Alice's object.
    let (status, _) = post_form(
        &app,
        "/updateMutableDescription",
        Some(&bob),
        &[("uri", PRIVATE_COLLECTION), ("value", "malicious")],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a non-owner edit is 403");

    // An anonymous caller may not edit it either.
    let (status, _) = post_form(
        &app,
        "/updateMutableDescription",
        None,
        &[("uri", PRIVATE_COLLECTION), ("value", "malicious")],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "an anonymous edit is 403");

    // The object is untouched: no mutable description was written.
    let descriptions = sparql_objects(&app, PRIVATE_COLLECTION, SBH_MUTABLE_DESCRIPTION).await;
    assert!(
        descriptions.is_empty(),
        "the rejected edits wrote nothing: {descriptions:?}"
    );
}

#[tokio::test]
async fn add_owner_grants_then_revokes_view() {
    let (app, _dir) = app().await;
    let alice = register_and_login(&app, "Alice Example", "alice").await;
    submit(&app, &alice).await;
    let bob = register_and_login(&app, "Bob Example", "bob").await;

    // Before the grant, Bob does not see Alice's private submission.
    let (status, manage) = get(&app, "/manage", Some(&bob)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !manage.contains(PRIVATE_COLLECTION),
        "Bob cannot see the object before addOwner: {manage}"
    );

    // Alice grants Bob ownership of the collection.
    let (status, _) = post_form(
        &app,
        "/user/alice/mysubmission/mysubmission_collection/1/addOwner",
        Some(&alice),
        &[("user", "bob")],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner addOwner succeeds");

    // Now Bob owns the object and sees it in /manage.
    let (status, manage) = get(&app, "/manage", Some(&bob)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        manage.contains(PRIVATE_COLLECTION),
        "Bob sees the object after addOwner: {manage}"
    );

    // Alice revokes Bob's ownership.
    let (status, _) = get(
        &app,
        "/user/alice/mysubmission/mysubmission_collection/1/removeOwner/bob",
        Some(&alice),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner removeOwner succeeds");

    // Bob no longer sees the object.
    let (status, manage) = get(&app, "/manage", Some(&bob)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !manage.contains(PRIVATE_COLLECTION),
        "Bob loses the object after removeOwner: {manage}"
    );
}
