//! HTTP-level tests for the V2 idiomatic resource surface over a SQLite backend,
//! driving the real axum router via `oneshot`. They pin the verb semantics that
//! distinguish V2 from V1: `POST /collections` creates, `PATCH /objects/{iri}`
//! edits in place, `DELETE /objects/{iri}` removes, and a `GET` never mutates.
//! Every mutating verb is identity-gated: an anonymous caller is `403`. All
//! delegate to the same `sbol-db-app` facade verbs the V1 adapter calls.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, Response, StatusCode};
use sbol_db_app::AppServices;
use sbol_db_backend::Backend;
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;

/// A compliant SBOL2 document (Turtle) submitted as the collection `content`: a
/// ComponentDefinition with a nested SequenceAnnotation, plus a Sequence.
const FIXTURE: &str = r#"
@prefix sbol: <http://sbols.org/v2#> .
@prefix dcterms: <http://purl.org/dc/terms/> .

<http://example.org/cd/1>
    a sbol:ComponentDefinition ;
    sbol:displayId "cd" ;
    sbol:persistentIdentity <http://example.org/cd> ;
    sbol:version "1" ;
    dcterms:title "My Component" .

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
    let path = dir.path().join("v2res.db");
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

/// Percent-encode an IRI into a single `/objects/{iri}` path segment.
fn encode_iri(iri: &str) -> String {
    iri.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

async fn body_string(res: Response<Body>) -> String {
    let bytes = to_bytes(res.into_body(), BODY_LIMIT).await.expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

/// Register a user through the V1 `/register` route and return a fresh token
/// from `/login`, usable as a V2 `Authorization: Bearer` credential (both
/// surfaces resolve tokens through the same store).
async fn register_and_login(app: &axum::Router, username: &str, email: &str) -> String {
    let form = serde_urlencoded::to_string([
        ("name", username),
        ("username", username),
        ("email", email),
        ("password", "s3cret"),
    ])
    .expect("encode form");
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
        .expect("register");
    assert_eq!(res.status(), StatusCode::OK, "registration succeeds");

    let form = serde_urlencoded::to_string([("email", email), ("password", "s3cret")])
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
        .expect("login");
    assert_eq!(res.status(), StatusCode::OK, "login succeeds");
    body_string(res).await.trim().to_owned()
}

/// Issue one request, returning the status, body, and any `Location` header.
async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    bearer: Option<&str>,
    content_type: Option<&str>,
    body: Option<String>,
) -> (StatusCode, String, Option<String>) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(ct) = content_type {
        builder = builder.header("content-type", ct);
    }
    let req = builder
        .body(body.map(Body::from).unwrap_or_else(Body::empty))
        .unwrap();
    let res = app.clone().oneshot(req).await.expect("request");
    let status = res.status();
    let location = res
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    (status, body_string(res).await, location)
}

/// Submit a collection through `POST /api/v2/collections` and return its minted
/// root Collection URI.
async fn create_collection(app: &axum::Router, token: &str, id: &str) -> String {
    let body = serde_json::json!({
        "id": id,
        "version": "1",
        "name": "A V2 submission",
        "format": "turtle",
        "content": FIXTURE,
    })
    .to_string();
    let (status, body, location) = send(
        app,
        "POST",
        "/api/v2/collections",
        Some(token),
        Some("application/json"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create body: {body}");
    assert!(
        location.is_some(),
        "a created collection carries a Location header"
    );
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    value["collection_uri"]
        .as_str()
        .expect("collection_uri")
        .to_owned()
}

#[tokio::test]
async fn post_collection_creates_a_readable_resource() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let collection = create_collection(&app, &token, "sub1").await;
    assert_eq!(
        collection,
        "http://synbiohub.org/user/alice/sub1/sub1_collection/1"
    );

    // The created collection's closure downloads back, proving POST wrote it.
    let uri = format!("/api/v2/objects/{}?format=sbol", encode_iri(&collection));
    let (status, body, _) = send(&app, "GET", &uri, Some(&token), None, None).await;
    assert_eq!(status, StatusCode::OK, "download body: {body}");
    assert!(!body.is_empty(), "the closure download is non-empty");
}

#[tokio::test]
async fn anonymous_create_is_forbidden() {
    let (app, _dir) = app().await;
    let body = serde_json::json!({ "id": "x", "version": "1", "content": FIXTURE }).to_string();
    let (status, _body, _) = send(
        &app,
        "POST",
        "/api/v2/collections",
        None,
        Some("application/json"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "anonymous POST is 403");
}

#[tokio::test]
async fn patch_edits_the_object_in_place() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let collection = create_collection(&app, &token, "sub2").await;
    let seg = encode_iri(&collection);

    let patch = serde_json::json!({ "mutable_description": "hello v2" }).to_string();
    let (status, body, _) = send(
        &app,
        "PATCH",
        &format!("/api/v2/objects/{seg}"),
        Some(&token),
        Some("application/json"),
        Some(patch),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "patch body: {body}");

    // The edit is visible in the object's verbatim RDF (Turtle negotiation).
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v2/objects/{seg}"))
                .header("authorization", format!("Bearer {token}"))
                .header("accept", "text/turtle")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("turtle read");
    assert_eq!(res.status(), StatusCode::OK);
    let turtle = body_string(res).await;
    assert!(
        turtle.contains("hello v2"),
        "the patched mutableDescription is present: {turtle}"
    );
}

#[tokio::test]
async fn anonymous_patch_is_forbidden() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let collection = create_collection(&app, &token, "sub3").await;
    let seg = encode_iri(&collection);

    let patch = serde_json::json!({ "mutable_description": "nope" }).to_string();
    let (status, _body, _) = send(
        &app,
        "PATCH",
        &format!("/api/v2/objects/{seg}"),
        None,
        Some("application/json"),
        Some(patch),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "anonymous PATCH is 403");
}

#[tokio::test]
async fn delete_removes_the_object_and_get_never_mutates() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let collection = create_collection(&app, &token, "sub4").await;
    let seg = encode_iri(&collection);
    let download = format!("/api/v2/objects/{seg}?format=sbol");

    // Two GETs do not remove the object: the second still succeeds, and the
    // subsequent DELETE proves the object was still present.
    for _ in 0..2 {
        let (status, _b, _) = send(&app, "GET", &download, Some(&token), None, None).await;
        assert_eq!(status, StatusCode::OK, "a GET does not mutate");
    }

    let (status, _b, _) = send(
        &app,
        "DELETE",
        &format!("/api/v2/objects/{seg}"),
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "DELETE returns 204");

    // The object is gone: its closure no longer resolves.
    let (status, _b, _) = send(&app, "GET", &download, Some(&token), None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "the deleted object is 404");
}

#[tokio::test]
async fn anonymous_delete_is_forbidden() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let collection = create_collection(&app, &token, "sub5").await;
    let seg = encode_iri(&collection);

    let (status, _b, _) = send(
        &app,
        "DELETE",
        &format!("/api/v2/objects/{seg}"),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "anonymous DELETE is 403");
}

#[tokio::test]
async fn publish_moves_a_private_object_to_the_public_graph() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let collection = create_collection(&app, &token, "sub6").await;
    let seg = encode_iri(&collection);

    let publish = serde_json::json!({ "id": "pub6", "version": "1" }).to_string();
    let (status, body, location) = send(
        &app,
        "POST",
        &format!("/api/v2/objects/{seg}/publish"),
        Some(&token),
        Some("application/json"),
        Some(publish),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "publish body: {body}");
    assert!(location.is_some(), "publish carries a Location header");
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    let public = value["collection_uri"].as_str().expect("collection_uri");
    assert!(
        public.starts_with("http://synbiohub.org/public/pub6/"),
        "published under the public prefix: {public}"
    );

    // The public object is now readable by an anonymous caller.
    let uri = format!("/api/v2/objects/{}?format=sbol", encode_iri(public));
    let (status, _b, _) = send(&app, "GET", &uri, None, None, None).await;
    assert_eq!(status, StatusCode::OK, "the published object is public");
}

#[tokio::test]
async fn list_and_search_return_a_paginated_envelope() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;

    let (status, body, _) = send(
        &app,
        "GET",
        "/api/v2/objects?limit=5&offset=0",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(value["items"].is_array(), "items is an array: {body}");
    assert!(value["total"].is_number(), "total is a number");
    assert_eq!(value["limit"], 5);
    assert_eq!(value["offset"], 0);

    let (status, body, _) = send(
        &app,
        "GET",
        "/api/v2/search?q=widget&limit=10",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(value["items"].is_array());
    assert!(value["total"].is_number());
}

#[tokio::test]
async fn private_object_is_hidden_from_an_anonymous_caller() {
    let (app, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let collection = create_collection(&app, &token, "sub7").await;
    let seg = encode_iri(&collection);

    // The owner reads it; an anonymous caller sees a non-disclosing 404.
    let (status, _b, _) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{seg}?format=sbol"),
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner reads the private object");

    let (status, _b, _) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{seg}?format=sbol"),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "hidden from anonymous");
}
