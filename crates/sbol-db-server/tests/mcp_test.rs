//! Authenticated MCP Streamable HTTP contract tests over the real application
//! facade and SQLite backend.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use sbol_db_app::{AppServices, Registration, SubmitRequest};
use sbol_db_backend::Backend;
use sbol_db_core::SerializationFormat;
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use sbol_db_storage::ImportOverwrite;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;

const FIXTURE: &str = r#"
@prefix sbol: <http://sbols.org/v2#> .
@prefix dcterms: <http://purl.org/dc/terms/> .

<http://example.org/cd/1>
    a sbol:ComponentDefinition ;
    sbol:displayId "cd" ;
    sbol:persistentIdentity <http://example.org/cd> ;
    sbol:version "1" ;
    dcterms:title "Private promoter" ;
    sbol:type <http://www.biopax.org/release/biopax-level3.owl#DnaRegion> ;
    sbol:role <http://identifiers.org/so/SO:0000167> .
"#;

async fn app() -> (Router, Arc<AppServices>, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mcp.db");
    let backend = Backend::open(&format!("sqlite://{}", path.display()))
        .await
        .expect("open sqlite backend");
    backend
        .migrator
        .as_ref()
        .expect("sqlite migrator")
        .run_migrations()
        .await
        .expect("run migrations");
    let mut config = ServerConfig {
        public_origin: Some("http://127.0.0.1:8888".to_owned()),
        mcp_enabled: true,
        ..ServerConfig::default()
    };
    config
        .resolve_public_origin("http://127.0.0.1:8888")
        .expect("valid public origin");
    let services = Arc::new(AppServices::from_backend(&backend));
    let state = AppState {
        service: backend.store.clone(),
        sparql: Arc::new(SparqlEngine::new(backend.triple_source.clone())),
        sparql_update: Arc::new(SparqlUpdateEngine::new(
            backend.triple_source.clone(),
            backend.triple_writer.clone(),
        )),
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

async fn register(services: &AppServices, username: &str) -> String {
    let user = services
        .auth
        .register(Registration {
            username: username.to_owned(),
            name: format!("{username} Example"),
            email: format!("{username}@example.org"),
            affiliation: None,
            password: "s3cret".to_owned(),
            is_admin: false,
            is_curator: false,
            is_member: true,
        })
        .await
        .expect("register user");
    services.auth.issue_token(user.id).await.expect("token")
}

async fn send(
    app: &Router,
    token: Option<&str>,
    origin: Option<&str>,
    body: Value,
) -> Response<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    app.clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .expect("response")
}

async fn json_body(response: Response<Body>) -> Value {
    let bytes = to_bytes(response.into_body(), BODY_LIMIT)
        .await
        .expect("body bytes");
    serde_json::from_slice(&bytes).expect("JSON response")
}

#[tokio::test]
async fn mcp_requires_a_live_bearer_and_rejects_cross_origin_requests() {
    let (app, services, _dir) = app().await;
    let token = register(&services, "alice").await;
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "1" }
        }
    });

    let response = send(&app, None, None, request.clone()).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers().contains_key("www-authenticate"));

    let response = send(&app, Some("not-a-live-token"), None, request.clone()).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = send(
        &app,
        Some(&token),
        Some("https://attacker.example"),
        request,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/mcp")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("GET response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mcp_initializes_and_lists_a_deterministic_read_only_tool_set() {
    let (app, services, _dir) = app().await;
    let token = register(&services, "alice").await;
    let response = send(
        &app,
        Some(&token),
        Some("http://127.0.0.1:8888"),
        json!({
            "jsonrpc": "2.0",
            "id": "init",
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" }
            }
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let initialize = json_body(response).await;
    assert_eq!(initialize["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(
        initialize["result"]["capabilities"]["tools"]["listChanged"],
        false
    );

    let tools = json_body(
        send(
            &app,
            Some(&token),
            None,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        )
        .await,
    )
    .await;
    let tools = tools["result"]["tools"].as_array().expect("tool array");
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["search_designs", "get_design", "validate_design_upload"]
    );
    assert!(tools
        .iter()
        .all(|tool| tool["annotations"]["readOnlyHint"] == true));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/mcp")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("GET response");
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers()["allow"], "POST");
}

#[tokio::test]
async fn mcp_reads_private_designs_only_for_the_authenticated_acl_scope() {
    let (app, services, _dir) = app().await;
    let alice = services
        .auth
        .register(Registration {
            username: "alice".to_owned(),
            name: "Alice Example".to_owned(),
            email: "alice@example.org".to_owned(),
            affiliation: None,
            password: "s3cret".to_owned(),
            is_admin: false,
            is_curator: false,
            is_member: true,
        })
        .await
        .unwrap();
    let alice_token = services.auth.issue_token(alice.id).await.unwrap();
    let bob_token = register(&services, "bob").await;
    let created = services
        .submission_service()
        .submit(SubmitRequest {
            owner: alice.username.clone(),
            id: "private_design".to_owned(),
            version: "1".to_owned(),
            name: Some("Private design".to_owned()),
            description: None,
            creator_name: Some(alice.name.clone()),
            citations: Vec::new(),
            body: FIXTURE.to_owned(),
            format: SerializationFormat::Turtle,
            overwrite: ImportOverwrite::Fail,
        })
        .await
        .expect("private submission");
    let iri = created.members[0].as_str();
    let call = |id: i32| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": "get_design", "arguments": { "iri": iri } }
        })
    };

    let visible = json_body(send(&app, Some(&alice_token), None, call(1)).await).await;
    assert_eq!(visible["result"]["isError"], false);
    assert_eq!(visible["result"]["structuredContent"]["iri"], iri);

    let hidden = json_body(send(&app, Some(&bob_token), None, call(2)).await).await;
    assert_eq!(hidden["result"]["isError"], true);
    assert!(hidden["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("not found or is not visible"));
}

#[tokio::test]
async fn mcp_validation_reports_the_exact_future_identity_without_writing() {
    let (app, services, _dir) = app().await;
    let token = register(&services, "alice").await;
    let response = send(
        &app,
        Some(&token),
        None,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "validate_design_upload",
                "arguments": {
                    "id": "agent_preview",
                    "version": "1",
                    "format": "turtle",
                    "collision": "fail",
                    "content": FIXTURE
                }
            }
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["result"]["isError"], false);
    assert_eq!(body["result"]["structuredContent"]["consequence"], "create");
    assert!(body["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("No registry data was changed"));

    let blank = json_body(
        send(
            &app,
            Some(&token),
            None,
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "validate_design_upload",
                    "arguments": { "id": "agent_preview", "content": "   " }
                }
            }),
        )
        .await,
    )
    .await;
    assert_eq!(blank["result"]["isError"], true);
    assert!(blank["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("content must not be empty"));
}
