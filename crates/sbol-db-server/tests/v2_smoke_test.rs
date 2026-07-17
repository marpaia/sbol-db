//! HTTP-level tests for the V2 REST surface over a SQLite backend, driving the
//! real axum router via `oneshot`. They exercise the slice-1 plumbing: the
//! bearer-token identity layer (a valid token widens the caller's graph scope;
//! a bad token is anonymous and public-only), the version probe, and the
//! consistent JSON error envelope.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use sbol_db_app::{AppServices, Registration};
use sbol_db_backend::Backend;
use sbol_db_core::{IriString, SerializationFormat};
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use sbol_db_storage::{ImportInput, ImportOverwrite};
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;

/// The private object minted into a graph alice owns. Only alice (or an admin)
/// may read it; an anonymous caller sees the public graph alone. The object IRI
/// is `namespace/displayId`, and the import's `document_iri` is the namespace,
/// so the owning named graph and the ACL scope line up.
const OBJECT_IRI: &str = "http://synbiohub.org/user/alice/priv/widget";
const OWNED_GRAPH: &str = "http://synbiohub.org/user/alice/priv";
const USER_GRAPH: &str = "http://synbiohub.org/user/alice";

/// An SBOL3 document holding one Component in alice's private graph, stamped
/// `sbh:ownedBy` her user graph so the ACL layer scopes it to her alone.
fn private_document() -> String {
    format!(
        r#"@prefix sbol: <http://sbols.org/v3#> .
@prefix sbh: <http://wiki.synbiohub.org/wiki/Terms/synbiohub#> .
@prefix SBO: <https://identifiers.org/SBO:> .
<{OBJECT_IRI}>
    a sbol:Component ;
    sbol:displayId "widget" ;
    sbol:name "Widget" ;
    sbol:description "A private widget" ;
    sbol:type SBO:0000251 ;
    sbol:hasNamespace <{OWNED_GRAPH}> ;
    sbh:ownedBy <{USER_GRAPH}> .
"#
    )
}

/// Percent-encode an IRI into a single path segment (colons and slashes
/// escaped) so it rides the `/objects/{iri}` route as one capture.
fn encode_iri(iri: &str) -> String {
    iri.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' => c.to_string(),
            other => {
                let mut buf = [0u8; 4];
                other
                    .encode_utf8(&mut buf)
                    .bytes()
                    .map(|b| format!("%{b:02X}"))
                    .collect()
            }
        })
        .collect()
}

/// Build a full [`AppState`] over a fresh SQLite backend, register alice, mint
/// her a token, and import the private owned document. Returns the assembled
/// app, alice's plaintext token, and the `TempDir` owning the database file.
async fn app_with_private_object() -> (axum::Router, String, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("v2.db");
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
    let app = Arc::new(AppServices::from_backend(&backend));
    let user = app
        .auth
        .register(Registration {
            username: "alice".to_owned(),
            name: "Alice".to_owned(),
            email: "alice@example.org".to_owned(),
            affiliation: None,
            password: "s3cret".to_owned(),
            is_admin: false,
            is_curator: false,
            is_member: true,
        })
        .await
        .expect("register");
    let token = app.auth.issue_token(user.id).await.expect("issue token");

    backend
        .store
        .import_document(ImportInput {
            body: private_document(),
            format: SerializationFormat::Turtle,
            namespace: None,
            source_uri: None,
            document_iri: Some(IriString::new(OWNED_GRAPH.to_owned()).expect("iri")),
            created_by: None,
            name: None,
            description: None,
            overwrite: ImportOverwrite::Fail,
        })
        .await
        .expect("import private document");

    let state = AppState {
        service: backend.store.clone(),
        sparql,
        sparql_update,
        app,
        metrics: Metrics::install(None, env!("CARGO_PKG_VERSION")),
        jobs: backend.jobs.clone(),
        lab: backend.lab.clone(),
        config: ServerConfig::default(),
        backend_kind: backend.kind,
        sql_console: backend.sql_console.clone(),
        db_stats: backend.db_stats.clone(),
        lsm_stats: backend.lsm_stats.clone(),
        schema_cache: Arc::new(SchemaCache::new()),
    };
    (router(state.clone(), ServerConfig::default()), token, dir)
}

async fn get(app: &axum::Router, uri: &str, bearer: Option<&str>) -> (StatusCode, String) {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let res = app
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .expect("request");
    let status = res.status();
    let bytes = to_bytes(res.into_body(), BODY_LIMIT).await.expect("body");
    (status, String::from_utf8(bytes.to_vec()).expect("utf8"))
}

#[tokio::test]
async fn v2_version_probe_is_public() {
    let (app, _token, _dir) = app_with_private_object().await;
    let (status, body) = get(&app, "/api/v2", None).await;
    assert_eq!(status, StatusCode::OK);
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(value["api"], "v2");
}

#[tokio::test]
async fn v2_bearer_token_authorizes_a_private_read() {
    let (app, token, _dir) = app_with_private_object().await;
    let uri = format!("/api/v2/objects/{}", encode_iri(OBJECT_IRI));

    // With alice's bearer token the object is inside her scope.
    let (status, body) = get(&app, &uri, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "authed read body: {body}");
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(value["iri"], OBJECT_IRI);
    assert_eq!(value["display_id"], "widget");
}

#[tokio::test]
async fn v2_bad_or_missing_token_is_anonymous_public_only() {
    let (app, _token, _dir) = app_with_private_object().await;
    let uri = format!("/api/v2/objects/{}", encode_iri(OBJECT_IRI));

    // No token: anonymous, scoped to public, so the private object is hidden.
    let (status, body) = get(&app, &uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "anon body: {body}");

    // A bad token is anonymous too (not a 401): same public-only visibility.
    let (status, _body) = get(&app, &uri, Some("not-a-real-token")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn v2_error_returns_json_envelope_with_status() {
    let (app, token, _dir) = app_with_private_object().await;
    let missing = encode_iri("http://synbiohub.org/public/nope/thing/1");
    let (status, body) = get(&app, &format!("/api/v2/objects/{missing}"), Some(&token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(value["error"]["code"], "not_found");
    assert_eq!(value["error"]["status"], 404);
    assert!(
        value["error"]["message"].is_string(),
        "envelope carries a human message: {body}"
    );
}
