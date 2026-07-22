//! HTTP-level integration tests for the SynBioHub V1 destructive object routes:
//! makePublic and the identity gate on remove.
//!
//! These drive the real axum router via `oneshot` over a SQLite-backed
//! [`AppState`]. An owner submits privately, makePublic's the submission, and the
//! response reports public-prefixed URIs; the private original is then gone from
//! the owner's `/manage` listing while the public collection is visible to an
//! anonymous caller. A non-owner (and an anonymous caller) attempting a remove is
//! rejected with `403`, proving the write-authorization gate holds.

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
const BOUNDARY: &str = "sboldbmakepublicboundary";

/// A compliant SBOL2 document (Turtle): a ComponentDefinition with a nested
/// SequenceAnnotation child, plus a standalone Sequence, all versioned `1`.
const FIXTURE: &str = r#"
@prefix sbol: <http://sbols.org/v2#> .
@prefix dcterms: <http://purl.org/dc/terms/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

<http://example.org/cd/1>
    a sbol:ComponentDefinition ;
    sbol:displayId "cd" ;
    sbol:persistentIdentity <http://example.org/cd> ;
    sbol:version "1" ;
    dcterms:title "My Component" ;
    sbol:type <http://www.biopax.org/release/biopax-level3.owl#DnaRegion> ;
    sbol:sequenceAnnotation <http://example.org/cd/anno/1> .

<http://example.org/cd/anno/1>
    a sbol:SequenceAnnotation ;
    sbol:displayId "anno" ;
    sbol:persistentIdentity <http://example.org/cd/anno> ;
    sbol:version "1" ;
    sbol:location <http://example.org/cd/anno/range/1> .

<http://example.org/cd/anno/range/1>
    a sbol:Range ;
    sbol:displayId "range" ;
    sbol:persistentIdentity <http://example.org/cd/anno/range> ;
    sbol:version "1" ;
    sbol:start "1"^^xsd:integer ;
    sbol:end "4"^^xsd:integer .

<http://example.org/seq/1>
    a sbol:Sequence ;
    sbol:displayId "seq" ;
    sbol:persistentIdentity <http://example.org/seq> ;
    sbol:version "1" ;
    sbol:elements "atgc" ;
    sbol:encoding <http://www.chem.qmul.ac.uk/iubmb/misc/naseq.html> .
"#;

/// Build a router over a fresh SQLite backend. The returned `TempDir` owns the
/// database file and must outlive the router.
async fn app() -> (axum::Router, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("makepublic.db");
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

/// A `multipart/form-data` submission body for the fixture document.
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

const PRIVATE_COLLECTION: &str =
    "http://synbiohub.org/user/alice/mysubmission/mysubmission_collection/1";
const PUBLIC_COLLECTION: &str = "http://synbiohub.org/public/mypublic/mypublic_collection/1";
const PUBLIC_COMPONENT: &str = "http://synbiohub.org/public/mypublic/cd/1";
const PUBLIC_SEQUENCE: &str = "http://synbiohub.org/public/mypublic/seq/1";

#[tokio::test]
async fn owner_make_public_publishes_and_removes_private() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "Alice Example", "alice").await;
    submit(&app, &token).await;

    // makePublic the submission's root collection into a fresh public collection.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/user/alice/mysubmission/mysubmission_collection/1/makePublic")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-authorization", &token)
                .body(Body::from("id=mypublic&version=1&tabState=new"))
                .unwrap(),
        )
        .await
        .expect("makePublic request");
    assert_eq!(res.status(), StatusCode::OK, "owner makePublic succeeds");
    let outcome: serde_json::Value =
        serde_json::from_str(&body_string(res).await).expect("makePublic json");

    assert_eq!(
        outcome["collectionUri"], PUBLIC_COLLECTION,
        "the response reports the minted public collection URI"
    );
    let members: Vec<&str> = outcome["members"]
        .as_array()
        .expect("members array")
        .iter()
        .map(|m| m.as_str().expect("member string"))
        .collect();
    assert!(
        members.contains(&PUBLIC_COMPONENT),
        "members include the public component: {members:?}"
    );
    assert!(
        members.contains(&PUBLIC_SEQUENCE),
        "members include the public sequence: {members:?}"
    );

    // The public collection is visible to an anonymous caller through the public
    // read scope.
    let (status, root) = get(&app, "/rootCollections", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        root.contains(PUBLIC_COLLECTION),
        "the public collection is listed for anonymous callers: {root}"
    );

    // The private original is gone: the owner's /manage listing no longer shows
    // the private submission.
    let (status, manage) = get(&app, "/manage", Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !manage.contains(PRIVATE_COLLECTION),
        "the private original is removed from /manage: {manage}"
    );
}

#[tokio::test]
async fn non_owner_remove_is_forbidden() {
    let (app, _dir) = app().await;
    let alice = register_and_login(&app, "Alice Example", "alice").await;
    submit(&app, &alice).await;

    // A different authenticated user may not remove Alice's object.
    let bob = register_and_login(&app, "Bob Example", "bob").await;
    let (status, _body) = get(
        &app,
        "/user/alice/mysubmission/mysubmission_collection/1/remove",
        Some(&bob),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a non-owner may not remove another user's object"
    );

    // An anonymous caller may not remove it either.
    let (status, _body) = get(
        &app,
        "/user/alice/mysubmission/mysubmission_collection/1/remove",
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "an anonymous caller may not remove an object"
    );

    // The object survives the rejected removes: the owner still sees it.
    let (status, manage) = get(&app, "/manage", Some(&alice)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        manage.contains(PRIVATE_COLLECTION),
        "the object is intact after the rejected removes: {manage}"
    );
}
