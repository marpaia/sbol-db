//! Authenticated MCP Streamable HTTP contract tests over the real application
//! facade and SQLite backend.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sbol_db_app::{AppServices, Registration, SubmitRequest};
use sbol_db_backend::Backend;
use sbol_db_core::{SerializationFormat, User};
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use sbol_db_storage::ImportOverwrite;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;
const MCP_RESOURCE: &str = "http://127.0.0.1:8888/mcp";

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

async fn register_user(services: &AppServices, username: &str) -> User {
    services
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
        .expect("register user")
}

async fn oauth_token(services: &AppServices, user: &User, scopes: &[&str]) -> String {
    let redirect_uri = "http://127.0.0.1:43123/callback";
    let client = services
        .oauth
        .register_public_client(
            "MCP integration test".to_owned(),
            vec![redirect_uri.to_owned()],
        )
        .await
        .expect("register OAuth client");
    let verifier = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let code = services
        .oauth
        .issue_authorization_code(
            user.id,
            &client.client_id,
            redirect_uri,
            MCP_RESOURCE,
            scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            &challenge,
            None,
        )
        .await
        .expect("issue authorization code");
    services
        .oauth
        .exchange_authorization_code(
            &code,
            &client.client_id,
            redirect_uri,
            Some(MCP_RESOURCE),
            verifier,
        )
        .await
        .expect("exchange authorization code")
        .access_token
}

async fn register(services: &AppServices, username: &str, scopes: &[&str]) -> String {
    let user = register_user(services, username).await;
    oauth_token(services, &user, scopes).await
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

async fn call_tool(app: &Router, token: &str, id: i32, name: &str, arguments: Value) -> Value {
    let response = send(
        app,
        Some(token),
        None,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK, "tool {name}");
    json_body(response).await
}

#[tokio::test]
async fn mcp_requires_a_live_bearer_and_rejects_cross_origin_requests() {
    let (app, services, _dir) = app().await;
    let token = register(&services, "alice", &["sbol:read"]).await;
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
async fn mcp_rejects_general_session_tokens_and_challenges_for_incremental_scope() {
    let (app, services, _dir) = app().await;
    let user = register_user(&services, "alice").await;
    let session_token = services.auth.issue_token(user.id).await.unwrap();
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2025-11-25" }
    });
    let response = send(&app, Some(&session_token), None, initialize).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let challenge = response.headers()["www-authenticate"].to_str().unwrap();
    assert!(challenge.contains("resource_metadata="));
    assert!(challenge.contains("sbol:read"));

    let read_token = oauth_token(&services, &user, &["sbol:read"]).await;
    let response = send(
        &app,
        Some(&read_token),
        None,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "upload_design_collection",
                "arguments": {}
            }
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let challenge = response.headers()["www-authenticate"].to_str().unwrap();
    assert!(challenge.contains("insufficient_scope"));
    assert!(challenge.contains("sbol:read sbol:write"));
}

#[tokio::test]
async fn mcp_initializes_and_lists_the_complete_capability_set() {
    let (app, services, _dir) = app().await;
    let token = register(&services, "alice", &["sbol:read"]).await;
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
        vec![
            "search_designs",
            "get_design",
            "download_design",
            "search_sequences",
            "find_similar_designs",
            "validate_design_upload",
            "upload_design_collection",
            "update_design_metadata",
            "publish_design",
            "list_design_collaborators",
            "share_design",
            "list_reviews",
            "start_design_review",
            "record_review_decision",
            "get_design_activity",
        ]
    );
    assert!(tools
        .iter()
        .any(|tool| tool["name"] == "upload_design_collection"
            && tool["annotations"]["readOnlyHint"] == false));
    assert!(tools.iter().any(
        |tool| tool["name"] == "search_designs" && tool["annotations"]["readOnlyHint"] == true
    ));

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
    let alice = register_user(&services, "alice").await;
    let alice_token = oauth_token(&services, &alice, &["sbol:read"]).await;
    let bob_token = register(&services, "bob", &["sbol:read"]).await;
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
    let token = register(&services, "alice", &["sbol:read"]).await;
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

#[tokio::test]
async fn mcp_executes_confirmed_contribution_sharing_review_and_publication_workflows() {
    let (app, services, _dir) = app().await;
    let alice = register_user(&services, "alice").await;
    let bob = register_user(&services, "bob").await;
    let curator = services
        .auth
        .register(Registration {
            username: "curator".to_owned(),
            name: "Curator Example".to_owned(),
            email: "curator@example.org".to_owned(),
            affiliation: None,
            password: "s3cret".to_owned(),
            is_admin: false,
            is_curator: true,
            is_member: true,
        })
        .await
        .unwrap();
    let alice_token = oauth_token(
        &services,
        &alice,
        &["sbol:read", "sbol:write", "sbol:share", "sbol:review"],
    )
    .await;
    let bob_token = oauth_token(&services, &bob, &["sbol:read"]).await;
    let curator_token = oauth_token(&services, &curator, &["sbol:read", "sbol:review"]).await;

    let upload = json!({
        "id": "agent_workflow",
        "version": "1",
        "name": "Agent workflow",
        "format": "turtle",
        "collision": "fail",
        "content": FIXTURE
    });
    let preview = call_tool(
        &app,
        &alice_token,
        1,
        "validate_design_upload",
        upload.clone(),
    )
    .await;
    assert_eq!(preview["result"]["isError"], false);
    let confirmation = preview["result"]["structuredContent"]["confirmation"].clone();

    let mut commit = upload.as_object().unwrap().clone();
    commit.insert("confirm".to_owned(), json!(true));
    commit.insert(
        "expected_collection_uri".to_owned(),
        confirmation["expected_collection_uri"].clone(),
    );
    commit.insert(
        "expected_consequence".to_owned(),
        confirmation["expected_consequence"].clone(),
    );
    let committed = call_tool(
        &app,
        &alice_token,
        2,
        "upload_design_collection",
        Value::Object(commit),
    )
    .await;
    assert_eq!(committed["result"]["isError"], false);
    let member = committed["result"]["structuredContent"]["members"][0]
        .as_str()
        .unwrap();

    let updated = call_tool(
        &app,
        &alice_token,
        3,
        "update_design_metadata",
        json!({
            "iri": member,
            "name": "Reviewed private promoter",
            "mutable_notes": "Prepared through the agent workflow",
            "confirm": true
        }),
    )
    .await;
    assert_eq!(updated["result"]["isError"], false);
    assert_eq!(
        updated["result"]["structuredContent"]["name"],
        "Reviewed private promoter"
    );

    let shared = call_tool(
        &app,
        &alice_token,
        4,
        "share_design",
        json!({ "iri": member, "user": "bob", "action": "grant", "confirm": true }),
    )
    .await;
    assert_eq!(shared["result"]["isError"], false);
    let bob_view = call_tool(&app, &bob_token, 5, "get_design", json!({ "iri": member })).await;
    assert_eq!(bob_view["result"]["isError"], false);

    let collaborators = call_tool(
        &app,
        &alice_token,
        6,
        "list_design_collaborators",
        json!({ "iri": member }),
    )
    .await;
    assert!(collaborators["result"]["structuredContent"]["viewers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|viewer| viewer["username"] == "bob"));

    let review = call_tool(
        &app,
        &alice_token,
        7,
        "start_design_review",
        json!({
            "iri": member,
            "curator": "curator",
            "note": "Please inspect the promoter metadata",
            "confirm": true
        }),
    )
    .await;
    assert_eq!(review["result"]["isError"], false);
    let decision = call_tool(
        &app,
        &curator_token,
        8,
        "record_review_decision",
        json!({
            "iri": member,
            "decision": "approve",
            "note": "Identity and provenance look correct",
            "confirm": true
        }),
    )
    .await;
    assert_eq!(decision["result"]["isError"], false);
    assert_eq!(
        decision["result"]["structuredContent"]["status"],
        "approved"
    );

    let activity = call_tool(
        &app,
        &alice_token,
        9,
        "get_design_activity",
        json!({ "iri": member }),
    )
    .await;
    assert!(
        activity["result"]["structuredContent"]["total"]
            .as_u64()
            .unwrap()
            >= 3
    );

    let published = call_tool(
        &app,
        &alice_token,
        10,
        "publish_design",
        json!({
            "iri": member,
            "id": "agent_public",
            "version": "1",
            "collision": "fail",
            "confirm": true
        }),
    )
    .await;
    assert_eq!(published["result"]["isError"], false);
    assert!(published["result"]["structuredContent"]["collection_uri"]
        .as_str()
        .unwrap()
        .contains("/public/agent_public/agent_public_collection/1"));
}
