//! The defining cross-surface data-parity test.
//!
//! V1 (the SynBioHub-compat adapter) and V2 (the idiomatic REST adapter) are two
//! presentations of the same `sbol-db-app` facade over one dataset. These tests
//! prove there is no divergence between the two views: an object written through
//! one surface reads back through the other, and the same query answered by both
//! surfaces returns the same object set.
//!
//! Both adapters are mounted on one `AppState`, so they share a single
//! `AppServices` (the same store, the same ranked index, the same identity). The
//! tests drive the real axum router via `oneshot` over a SQLite backend and mix
//! V1 and V2 calls against it.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, Response, StatusCode};
use sbol_db_app::AppServices;
use sbol_db_backend::Backend;
use sbol_db_search::ranked_text::{IndexedPart, RankedTextIndex};
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use serde_json::Value;
use std::collections::BTreeSet;
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;
const BOUNDARY: &str = "sboldbparityboundary";
const PUBLIC_GRAPH: &str = "http://synbiohub.org/public";

/// A compliant SBOL2 document (Turtle): a titled ComponentDefinition plus a
/// standalone Sequence, both versioned `1`.
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

/// Build a router over a fresh SQLite backend, returning the shared ranked text
/// index so a test can seed one corpus both surfaces then read. The `TempDir`
/// owns the database file and must outlive the router.
async fn app() -> (axum::Router, Arc<RankedTextIndex>, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("v2parity.db");
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
    let text_index = Arc::new(RankedTextIndex::in_ram().expect("in-ram index"));
    let app_services = AppServices::from_backend(&backend).with_text_search(text_index.clone());
    let config = ServerConfig::default();
    let state = AppState {
        service: backend.store.clone(),
        sparql,
        sparql_update,
        app: Arc::new(app_services),
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
    (router(state, config), text_index, dir)
}

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

/// Register a user (V1 `/register`) and return a fresh login token. The same
/// token authenticates both surfaces: V1 reads it from `X-authorization`, V2
/// from `Authorization: Bearer`, and both resolve it through the one store.
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

/// A `multipart/form-data` body carrying the V1 submission form and SBOL file.
fn v1_submit_body(id: &str) -> String {
    let field = |name: &str, value: &str| {
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        )
    };
    let mut body = String::new();
    body.push_str(&field("id", id));
    body.push_str(&field("version", "1"));
    body.push_str(&field("name", "A V1 submission"));
    body.push_str(&field("format", "turtle"));
    body.push_str(&format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"doc.ttl\"\r\nContent-Type: text/turtle\r\n\r\n{FIXTURE}\r\n"
    ));
    body.push_str(&format!("--{BOUNDARY}--\r\n"));
    body
}

/// Submit through the V1 `POST /submit` route (multipart, `X-authorization`) and
/// return the minted root Collection URI.
async fn v1_submit(app: &axum::Router, token: &str, id: &str) -> String {
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
                .body(Body::from(v1_submit_body(id)))
                .unwrap(),
        )
        .await
        .expect("v1 submit");
    assert_eq!(res.status(), StatusCode::OK, "V1 submit succeeds");
    // Classic SynBioHub's V1 submit answers with a bare text/plain ack, not the
    // minted URI, so the caller derives the deterministic collection URI from
    // the submission id under alice's namespace.
    assert_eq!(body_string(res).await.trim(), "Successfully uploaded");
    format!("http://synbiohub.org/user/alice/{id}/{id}_collection/1")
}

/// Create through the V2 `POST /api/v2/collections` route (JSON body) and return
/// the minted root Collection URI.
async fn v2_create(app: &axum::Router, token: &str, id: &str) -> String {
    let body = serde_json::json!({
        "id": id,
        "version": "1",
        "name": "A V2 submission",
        "format": "turtle",
        "content": FIXTURE,
    })
    .to_string();
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v2/collections")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("v2 create");
    assert_eq!(res.status(), StatusCode::CREATED, "V2 create succeeds");
    let outcome: Value = serde_json::from_str(&body_string(res).await).expect("create json");
    outcome["collection_uri"]
        .as_str()
        .expect("collection_uri")
        .to_owned()
}

/// GET a path with an optional bearer token, returning status and body.
async fn get(app: &axum::Router, uri: &str, bearer: Option<&str>) -> (StatusCode, String) {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let res = app
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .expect("get");
    let status = res.status();
    (status, body_string(res).await)
}

/// GET a V1 path with an optional `X-authorization` identity.
async fn get_v1(app: &axum::Router, uri: &str, token: Option<&str>) -> (StatusCode, String) {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(token) = token {
        builder = builder.header("x-authorization", token);
    }
    let res = app
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .expect("get v1");
    let status = res.status();
    (status, body_string(res).await)
}

#[tokio::test]
async fn object_submitted_via_v1_reads_back_through_v2() {
    let (app, _index, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;

    // Written through the V1 SynBioHub `/submit` route.
    let collection = v1_submit(&app, &token, "fromv1").await;
    assert_eq!(
        collection,
        "http://synbiohub.org/user/alice/fromv1/fromv1_collection/1"
    );
    let component = "http://synbiohub.org/user/alice/fromv1/cd/1";

    // Read back through the V2 idiomatic route: the collection's closure
    // downloads, proving the V1 write is visible to V2.
    let (status, body) = get(
        &app,
        &format!("/api/v2/objects/{}?format=sbol", encode_iri(&collection)),
        Some(&token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "V2 download of the V1 object: {body}"
    );
    assert!(!body.is_empty(), "the closure is non-empty");

    // The V1-minted component member reads back through V2 as RDF at its minted
    // URI, confirming the whole submission (not just the root) crossed surfaces.
    let (status, turtle) = {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/v2/objects/{}", encode_iri(component)))
                    .header("authorization", format!("Bearer {token}"))
                    .header("accept", "text/turtle")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("v2 turtle read");
        let status = res.status();
        (status, body_string(res).await)
    };
    assert_eq!(status, StatusCode::OK, "V2 RDF read of the V1 member");
    assert!(
        turtle.contains(component),
        "the member's RDF names its minted URI: {turtle}"
    );
}

#[tokio::test]
async fn object_created_via_v2_reads_back_through_v1() {
    let (app, _index, _dir) = app().await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;

    // Written through the V2 idiomatic `POST /api/v2/collections` route.
    let collection = v2_create(&app, &token, "fromv2").await;
    assert_eq!(
        collection,
        "http://synbiohub.org/user/alice/fromv2/fromv2_collection/1"
    );

    // Read back through the V1 SynBioHub metadata route for the root collection.
    let (status, body) = get_v1(
        &app,
        "/user/alice/fromv2/fromv2_collection/1/metadata",
        Some(&token),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "V1 metadata of the V2 object: {body}"
    );
    assert!(
        body.contains("Collection"),
        "the collection's type surfaces through V1: {body}"
    );

    // The V2-minted component member reads back through V1 metadata at its minted
    // URI, carrying the title written on submission.
    let (status, body) = get_v1(&app, "/user/alice/fromv2/cd/1/metadata", Some(&token)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "V1 metadata of the V2 member: {body}"
    );
    assert!(
        body.contains("ComponentDefinition"),
        "the member's type surfaces through V1: {body}"
    );
    assert!(
        body.contains("My Component"),
        "the member's title surfaces through V1: {body}"
    );
}

/// The object URIs in classic's V1 `/search` JSON array (each row's `uri`).
fn v1_subjects(body: &str) -> BTreeSet<String> {
    let value: Value = serde_json::from_str(body).expect("V1 search JSON array");
    value
        .as_array()
        .expect("search results array")
        .iter()
        .filter_map(|row| row["uri"].as_str().map(str::to_owned))
        .collect()
}

/// The `uri`s in a V2 search response envelope.
fn v2_uris(body: &str) -> BTreeSet<String> {
    let value: Value = serde_json::from_str(body).expect("V2 search JSON");
    value["items"]
        .as_array()
        .expect("items array")
        .iter()
        .filter_map(|item| item["uri"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn same_query_returns_the_same_object_set_on_both_surfaces() {
    let (app, index, _dir) = app().await;

    // Seed one public corpus into the shared ranked index both surfaces read.
    // Two objects match the term, one does not.
    let cd = "http://sbols.org/v2#ComponentDefinition";
    index
        .rebuild(vec![
            IndexedPart {
                subject: "http://synbiohub.org/public/promoterA/1".to_owned(),
                graph: PUBLIC_GRAPH.to_owned(),
                display_id: Some("promoterA".to_owned()),
                name: Some("GFP promoter".to_owned()),
                description: Some("a strong promoter".to_owned()),
                version: Some("1".to_owned()),
                type_iris: vec![cd.to_owned()],
                keywords: String::new(),
                pagerank: 1.0,
            },
            IndexedPart {
                subject: "http://synbiohub.org/public/promoterB/1".to_owned(),
                graph: PUBLIC_GRAPH.to_owned(),
                display_id: Some("promoterB".to_owned()),
                name: Some("RFP promoter".to_owned()),
                description: Some("another promoter".to_owned()),
                version: Some("1".to_owned()),
                type_iris: vec![cd.to_owned()],
                keywords: String::new(),
                pagerank: 1.0,
            },
            IndexedPart {
                subject: "http://synbiohub.org/public/terminatorC/1".to_owned(),
                graph: PUBLIC_GRAPH.to_owned(),
                display_id: Some("terminatorC".to_owned()),
                name: Some("a terminator".to_owned()),
                description: Some("stops transcription".to_owned()),
                version: Some("1".to_owned()),
                type_iris: vec![cd.to_owned()],
                keywords: String::new(),
                pagerank: 1.0,
            },
        ])
        .expect("seed ranked index");

    // The V1 free-text `/search/<term>` and the V2 `/api/v2/search?q=<term>`
    // both delegate to the one ranked-search facade verb over the one index.
    let (v1_status, v1_body) = get_v1(&app, "/search/promoter", None).await;
    assert_eq!(v1_status, StatusCode::OK, "V1 search: {v1_body}");
    let (v2_status, v2_body) = get(&app, "/api/v2/search?q=promoter", None).await;
    assert_eq!(v2_status, StatusCode::OK, "V2 search: {v2_body}");

    let via_v1 = v1_subjects(&v1_body);
    let via_v2 = v2_uris(&v2_body);

    assert_eq!(
        via_v1, via_v2,
        "V1 and V2 return the same object set for the same query"
    );
    assert_eq!(
        via_v1,
        BTreeSet::from([
            "http://synbiohub.org/public/promoterA/1".to_owned(),
            "http://synbiohub.org/public/promoterB/1".to_owned(),
        ]),
        "the two matching objects, not the terminator"
    );
}
