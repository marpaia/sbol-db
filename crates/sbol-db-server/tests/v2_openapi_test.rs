//! Contract tests for the V2 OpenAPI surface.
//!
//! These drive the real axum router via `oneshot` over a SQLite backend. They
//! prove the spec is served at `/api/v2/openapi.json` (with the docs page at
//! `/api/v2/docs`) and that representative V2 responses conform to the schema
//! the spec itself declares. The schema is pulled from the served document, so
//! the check fails if a handler's shape and its documented contract drift apart.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, Response, StatusCode};
use sbol_db_app::AppServices;
use sbol_db_backend::Backend;
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;

/// A compliant SBOL2 document (Turtle) submitted as a collection `content`.
const FIXTURE: &str = r#"
@prefix sbol: <http://sbols.org/v2#> .
@prefix dcterms: <http://purl.org/dc/terms/> .

<http://example.org/cd/1>
    a sbol:ComponentDefinition ;
    sbol:displayId "cd" ;
    sbol:persistentIdentity <http://example.org/cd> ;
    sbol:version "1" ;
    dcterms:title "My Component" ;
    sbol:type <http://www.biopax.org/release/biopax-level3.owl#DnaRegion> .

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
async fn app_with_services() -> (axum::Router, Arc<AppServices>, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("v2openapi.db");
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
    let services = Arc::new(AppServices::from_backend(&backend));
    let state = AppState {
        service: backend.store.clone(),
        sparql,
        sparql_update,
        app: services.clone(),
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
    (router(state, config), services, dir)
}

async fn app() -> (axum::Router, TempDir) {
    let (app, _services, dir) = app_with_services().await;
    (app, dir)
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

/// Issue one request, returning the status and body.
async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    bearer: Option<&str>,
    content_type: Option<&str>,
    body: Option<String>,
) -> (StatusCode, String) {
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
    (status, body_string(res).await)
}

/// Fetch and parse the served OpenAPI document.
async fn openapi(app: &axum::Router) -> Value {
    let (status, body) = send(app, "GET", "/api/v2/openapi.json", None, None, None).await;
    assert_eq!(status, StatusCode::OK, "openapi.json is served");
    serde_json::from_str(&body).expect("openapi.json is valid JSON")
}

// --- a minimal JSON Schema validator over the subset the spec uses -----------
//
// Handles `$ref`, `type` (as a string or an array of strings, including
// `"null"`), `required`, `properties`, and `items`. Unknown properties pass, as
// JSON Schema defaults to `additionalProperties: true`. This is enough to
// validate every representative response the spec declares.

/// Resolve a local `#/a/b/c` JSON pointer against the root document.
fn resolve_ref<'a>(root: &'a Value, reference: &str) -> &'a Value {
    let path = reference
        .strip_prefix("#/")
        .unwrap_or_else(|| panic!("only local refs are supported: {reference}"));
    let mut node = root;
    for segment in path.split('/') {
        node = node
            .get(segment)
            .unwrap_or_else(|| panic!("dangling $ref segment '{segment}' in {reference}"));
    }
    node
}

/// Whether `value` satisfies one named JSON type.
fn matches_type(value: &Value, ty: &str) -> bool {
    match ty {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        other => panic!("unknown schema type: {other}"),
    }
}

/// Validate `value` against `schema`, returning `Err(path-qualified message)` on
/// the first violation.
fn validate(value: &Value, schema: &Value, root: &Value, path: &str) -> Result<(), String> {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        return validate(value, resolve_ref(root, reference), root, path);
    }

    if let Some(ty) = schema.get("type") {
        let ok = match ty {
            Value::String(name) => matches_type(value, name),
            Value::Array(names) => names
                .iter()
                .any(|n| matches_type(value, n.as_str().expect("type name is a string"))),
            other => panic!("unexpected `type` shape: {other}"),
        };
        if !ok {
            return Err(format!("{path}: {value} does not match type {ty}"));
        }
    }

    if value.is_object() {
        let object = value.as_object().unwrap();
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for field in required {
                let field = field.as_str().expect("required entry is a string");
                if !object.contains_key(field) {
                    return Err(format!("{path}: missing required field '{field}'"));
                }
            }
        }
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (name, subschema) in properties {
                if let Some(child) = object.get(name) {
                    validate(child, subschema, root, &format!("{path}.{name}"))?;
                }
            }
        }
    }

    if let Some(items) = value.as_array() {
        if let Some(item_schema) = schema.get("items") {
            for (index, item) in items.iter().enumerate() {
                validate(item, item_schema, root, &format!("{path}[{index}]"))?;
            }
        }
    }

    Ok(())
}

/// The JSON response schema the spec declares for `path`/`method`/`status`,
/// resolving a response `$ref` to its `application/json` schema.
fn response_schema<'a>(spec: &'a Value, path: &str, method: &str, status: &str) -> &'a Value {
    let response = &spec["paths"][path][method]["responses"][status];
    let response = match response.get("$ref").and_then(Value::as_str) {
        Some(reference) => resolve_ref(spec, reference),
        None => response,
    };
    &response["content"]["application/json"]["schema"]
}

/// Assert a served body conforms to the declared schema for an operation.
fn assert_conforms(spec: &Value, path: &str, method: &str, status: &str, body: &str) {
    let value: Value = serde_json::from_str(body).expect("response is JSON");
    let schema = response_schema(spec, path, method, status);
    validate(&value, schema, spec, "$")
        .unwrap_or_else(|e| panic!("{method} {path} ({status}) violates its schema: {e}\n{body}"));
}

async fn create_collection(app: &axum::Router, token: &str, id: &str) -> (String, String) {
    let body = serde_json::json!({
        "id": id,
        "version": "1",
        "name": "A V2 submission",
        "format": "turtle",
        "content": FIXTURE,
    })
    .to_string();
    let (status, body) = send(
        app,
        "POST",
        "/api/v2/collections",
        Some(token),
        Some("application/json"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create body: {body}");
    let value: Value = serde_json::from_str(&body).expect("json");
    let uri = value["collection_uri"]
        .as_str()
        .expect("collection_uri")
        .to_owned();
    (uri, body)
}

#[tokio::test]
async fn openapi_json_is_served_and_wellformed() {
    let (app, _dir) = app().await;
    let spec = openapi(&app).await;

    assert_eq!(spec["openapi"], "3.1.0", "declares OpenAPI 3.1");
    assert_eq!(spec["servers"][0]["url"], "/api/v2", "based at /api/v2");

    // Every documented path is present.
    for path in [
        "/",
        "/instance",
        "/session",
        "/account",
        "/account/password",
        "/account/shared",
        "/reviews",
        "/objects",
        "/objects/{iri}",
        "/objects/{iri}/details",
        "/objects/{iri}/activity",
        "/objects/{iri}/reviews",
        "/objects/{iri}/reviews/decision",
        "/objects/{iri}/shares",
        "/objects/{iri}/shares/{user}",
        "/objects/{iri}/owner",
        "/objects/{iri}/publish",
        "/objects/{iri}/similar",
        "/collections",
        "/collections/validate",
        "/collections/{iri}",
        "/collections/{iri}/content",
        "/collections/{iri}/members",
        "/collections/{iri}/members/{member}",
        "/admin",
        "/admin/instance",
        "/admin/users",
        "/admin/users/{username}",
        "/admin/integrations",
        "/admin/federation",
        "/admin/federation/sync",
        "/admin/registries",
        "/admin/registries/{uri}",
        "/admin/remotes",
        "/admin/remotes/{id}",
        "/admin/plugins",
        "/admin/plugins/{category}/{id}",
        "/admin/jobs",
        "/admin/jobs/{id}",
        "/admin/jobs/{id}/attempts",
        "/admin/jobs/{id}/logs",
        "/admin/jobs/{id}/cancel",
        "/admin/ontologies",
        "/admin/search",
        "/admin/search/rebuild",
        "/admin/backup",
        "/admin/backup/validate",
        "/admin/backup/restore",
        "/admin/audit",
        "/search",
        "/search/facets",
        "/search/strategies",
        "/sequences/search",
    ] {
        assert!(
            spec["paths"].get(path).is_some(),
            "the spec documents {path}"
        );
    }

    // The verb semantics that distinguish V2 are declared: DELETE and PATCH on
    // the object path, POST (not GET) for creation, and no mutating GET.
    let object = &spec["paths"]["/objects/{iri}"];
    assert!(object.get("get").is_some(), "GET reads an object");
    assert!(object.get("patch").is_some(), "PATCH edits an object");
    assert!(object.get("delete").is_some(), "DELETE removes an object");
    assert!(
        spec["paths"]["/collections"].get("post").is_some(),
        "POST creates a collection"
    );
    assert!(
        spec["paths"]["/collections/{iri}/content"]
            .get("get")
            .is_some()
            && spec["paths"]["/collections/{iri}/content"]
                .get("put")
                .is_some(),
        "collection synchronization content is read and replaced through GET/PUT"
    );

    // API and browser auth are both declared; bearer is selected first by the
    // implementation when both are presented.
    assert_eq!(
        spec["components"]["securitySchemes"]["bearerAuth"]["scheme"],
        "bearer"
    );
    assert_eq!(
        spec["components"]["securitySchemes"]["cookieAuth"]["name"],
        "sbol-db-token"
    );
    // The shared error envelope schema exists.
    assert!(spec["components"]["schemas"]["Error"]["properties"]["error"].is_object());
}

#[tokio::test]
async fn docs_page_is_served() {
    let (app, _dir) = app().await;
    let (status, body) = send(&app, "GET", "/api/v2/docs", None, None, None).await;
    assert_eq!(status, StatusCode::OK, "the docs page is served");
    assert!(
        body.contains("/api/v2/openapi.json"),
        "the docs page points at the V2 spec"
    );
}

#[tokio::test]
async fn version_response_matches_schema() {
    let (app, _dir) = app().await;
    let spec = openapi(&app).await;
    let (status, body) = send(&app, "GET", "/api/v2", None, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_conforms(&spec, "/", "get", "200", &body);
}

#[tokio::test]
async fn portal_bootstrap_responses_match_schema() {
    let (app, _dir) = app().await;
    let spec = openapi(&app).await;

    let (status, body) = send(&app, "GET", "/api/v2/instance", None, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_conforms(&spec, "/instance", "get", "200", &body);

    let (status, body) = send(&app, "GET", "/api/v2/session", None, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_conforms(&spec, "/session", "get", "200", &body);

    let _token = register_and_login(&app, "alice", "alice@example.org").await;
    let login = serde_json::json!({
        "identifier": "alice@example.org",
        "password": "s3cret"
    })
    .to_string();
    let (status, body) = send(
        &app,
        "POST",
        "/api/v2/session",
        None,
        Some("application/json"),
        Some(login),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_conforms(&spec, "/session", "post", "200", &body);
}

#[tokio::test]
async fn account_and_collaboration_responses_match_schema() {
    let (app, services, _dir) = app_with_services().await;
    let spec = openapi(&app).await;
    let alice = register_and_login(&app, "alice", "alice@example.org").await;
    let bob = register_and_login(&app, "bob", "bob@example.org").await;
    let mut bob_user = services
        .users
        .find_by_email_or_username("bob")
        .await
        .expect("bob lookup")
        .expect("bob");
    bob_user.is_curator = true;
    services
        .users
        .update_user(&bob_user)
        .await
        .expect("grant curator role");

    let (status, body) = send(&app, "GET", "/api/v2/account", Some(&alice), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_conforms(&spec, "/account", "get", "200", &body);

    let (status, body) = send(
        &app,
        "PATCH",
        "/api/v2/account",
        Some(&alice),
        Some("application/json"),
        Some(serde_json::json!({ "affiliation": "Test Lab" }).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_conforms(&spec, "/account", "patch", "200", &body);

    let (collection, _) = create_collection(&app, &alice, "collaboration-contract").await;
    let encoded = encode_iri(&collection);
    let (status, body) = send(
        &app,
        "POST",
        &format!("/api/v2/objects/{encoded}/shares"),
        Some(&alice),
        Some("application/json"),
        Some(serde_json::json!({ "user": "bob" }).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "share body: {body}");

    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{encoded}/shares"),
        Some(&alice),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_conforms(&spec, "/objects/{iri}/shares", "get", "200", &body);

    let (status, body) = send(
        &app,
        "GET",
        "/api/v2/account/shared",
        Some(&bob),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_conforms(&spec, "/account/shared", "get", "200", &body);

    let (status, body) = send(
        &app,
        "POST",
        &format!("/api/v2/objects/{encoded}/reviews"),
        Some(&alice),
        Some("application/json"),
        Some(serde_json::json!({ "curator": "bob" }).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "review body: {body}");
    assert_conforms(&spec, "/objects/{iri}/reviews", "post", "201", &body);

    let (status, body) = send(&app, "GET", "/api/v2/reviews", Some(&bob), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_conforms(&spec, "/reviews", "get", "200", &body);

    let (status, body) = send(
        &app,
        "POST",
        &format!("/api/v2/objects/{encoded}/reviews/decision"),
        Some(&bob),
        Some("application/json"),
        Some(serde_json::json!({ "decision": "approve" }).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "decision body: {body}");
    assert_conforms(
        &spec,
        "/objects/{iri}/reviews/decision",
        "post",
        "200",
        &body,
    );

    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{encoded}/activity"),
        Some(&alice),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_conforms(&spec, "/objects/{iri}/activity", "get", "200", &body);
}

#[tokio::test]
async fn search_response_matches_schema() {
    let (app, _dir) = app().await;
    let spec = openapi(&app).await;

    let (status, body) = send(&app, "GET", "/api/v2/search?q=component", None, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_conforms(&spec, "/search", "get", "200", &body);

    // The object list shares the same envelope schema.
    let (status, body) = send(&app, "GET", "/api/v2/objects?limit=5", None, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_conforms(&spec, "/objects", "get", "200", &body);

    let (status, body) = send(
        &app,
        "POST",
        "/api/v2/search",
        None,
        Some("application/json"),
        Some(
            serde_json::json!({
                "query": { "kind": "text", "text": "component" }
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_conforms(&spec, "/search", "post", "200", &body);

    let (status, body) = send(&app, "GET", "/api/v2/search/strategies", None, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_conforms(&spec, "/search/strategies", "get", "200", &body);

    let (status, body) = send(&app, "GET", "/api/v2/search/facets", None, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_conforms(&spec, "/search/facets", "get", "200", &body);
}

#[tokio::test]
async fn error_response_matches_schema() {
    let (app, _dir) = app().await;
    let spec = openapi(&app).await;

    // A missing object is a 404 in the V2 envelope.
    let missing = encode_iri("http://synbiohub.org/public/nope/1");
    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{missing}"),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_conforms(&spec, "/objects/{iri}", "get", "404", &body);

    // Malformed query values are parsed inside the V2 handler so clients
    // always receive the documented JSON envelope, never Axum's plain text.
    let (status, body) = send(
        &app,
        "GET",
        "/api/v2/sequences/search?q=ATGC&limit=not-a-number",
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_conforms(&spec, "/sequences/search", "get", "400", &body);
    let error: Value = serde_json::from_str(&body).expect("V2 error JSON");
    assert_eq!(error["error"]["code"], "invalid_input");
}

#[tokio::test]
async fn collection_created_matches_schema() {
    let (app, _dir) = app().await;
    let spec = openapi(&app).await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let (_uri, body) = create_collection(&app, &token, "sub1").await;
    assert_conforms(&spec, "/collections", "post", "201", &body);
}

#[tokio::test]
async fn contribution_preview_matches_schema() {
    let (app, _dir) = app().await;
    let spec = openapi(&app).await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let request = serde_json::json!({
        "id": "preview",
        "version": "1",
        "format": "turtle",
        "content": FIXTURE,
    })
    .to_string();
    let (status, body) = send(
        &app,
        "POST",
        "/api/v2/collections/validate",
        Some(&token),
        Some("application/json"),
        Some(request),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "preview body: {body}");
    assert_conforms(&spec, "/collections/validate", "post", "200", &body);
}

#[tokio::test]
async fn similar_response_matches_schema() {
    let (app, _dir) = app().await;
    let spec = openapi(&app).await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let (uri, _) = create_collection(&app, &token, "sub2").await;
    let seg = encode_iri(&uri);
    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{seg}/similar"),
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_conforms(&spec, "/objects/{iri}/similar", "get", "200", &body);
}

#[tokio::test]
async fn object_details_response_matches_schema() {
    let (app, _dir) = app().await;
    let spec = openapi(&app).await;
    let token = register_and_login(&app, "alice", "alice@example.org").await;
    let (uri, _) = create_collection(&app, &token, "details").await;
    let segment = encode_iri(&uri);
    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v2/objects/{segment}/details"),
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "details body: {body}");
    assert_conforms(&spec, "/objects/{iri}/details", "get", "200", &body);
}
