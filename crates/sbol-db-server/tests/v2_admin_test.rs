//! Native administrator boundary and integrity workflow tests.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use sbol_db_app::{AppServices, Registration};
use sbol_db_backend::Backend;
use sbol_db_core::{IriString, SerializationFormat, User};
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use sbol_db_storage::{ImportInput, ImportOverwrite};
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;
const DOCUMENT: &str = r#"
@prefix sbol: <http://sbols.org/v3#> .
<https://example.org/component> a sbol:Component ;
  sbol:displayId "component" ;
  sbol:name "Backup component" ;
  sbol:type <https://identifiers.org/SBO:0000251> .
"#;

async fn app() -> (Router, Arc<AppServices>, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("v2-admin.db");
    let backend = Backend::open(&format!("sqlite://{}", path.display()))
        .await
        .expect("open sqlite");
    backend
        .migrator
        .as_ref()
        .expect("migrator")
        .run_migrations()
        .await
        .expect("migrate");
    let services = Arc::new(AppServices::from_backend(&backend));
    let config = ServerConfig::default();
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

async fn register(services: &AppServices, username: &str, is_admin: bool) -> (User, String) {
    let user = services
        .auth
        .register(Registration {
            username: username.to_owned(),
            name: format!("{username} name"),
            email: format!("{username}@example.org"),
            affiliation: None,
            password: "s3cret".to_owned(),
            is_admin,
            is_curator: is_admin,
            is_member: true,
        })
        .await
        .expect("register");
    let token = services.auth.issue_token(user.id).await.expect("token");
    (user, token)
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> Response<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    app.clone()
        .oneshot(builder.body(Body::from(body.to_string())).expect("request"))
        .await
        .expect("response")
}

async fn body(response: Response<Body>) -> Value {
    let bytes = to_bytes(response.into_body(), BODY_LIMIT)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json")
}

#[tokio::test]
async fn one_admin_policy_gates_safe_instance_and_user_management() {
    let (app, services, _dir) = app().await;
    let (_admin, admin_token) = register(&services, "admin", true).await;
    let (_member, member_token) = register(&services, "member", false).await;

    let anonymous = send(&app, "GET", "/api/v2/admin", None, json!({})).await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(body(anonymous).await["error"]["code"], "unauthorized");

    let member = send(&app, "GET", "/api/v2/admin", Some(&member_token), json!({})).await;
    assert_eq!(member.status(), StatusCode::FORBIDDEN);

    let overview = send(&app, "GET", "/api/v2/admin", Some(&admin_token), json!({})).await;
    assert_eq!(overview.status(), StatusCode::OK);
    assert_eq!(
        body(overview).await["policy"],
        "authenticated_administrator"
    );

    let updated = send(
        &app,
        "PATCH",
        "/api/v2/admin/instance",
        Some(&admin_token),
        json!({
            "name": "University of Colorado Boulder",
            "uri_prefix": "https://sbol-db.colorado.edu/",
            "allow_public_signup": false
        }),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    assert_eq!(
        body(updated).await["name"],
        "University of Colorado Boulder"
    );

    let created = send(
        &app,
        "POST",
        "/api/v2/admin/users",
        Some(&admin_token),
        json!({
            "username": "curator",
            "name": "Curator",
            "email": "curator@example.org",
            "password": "temporary",
            "is_curator": true,
            "is_member": true
        }),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = body(created).await;
    assert_eq!(created["username"], "curator");
    let encoded = created.to_string();
    assert!(!encoded.contains("temporary"));
    assert!(!encoded.contains("password_hash"));
    assert!(!encoded.contains("reset"));

    let users = body(
        send(
            &app,
            "GET",
            "/api/v2/admin/users",
            Some(&admin_token),
            json!({}),
        )
        .await,
    )
    .await;
    assert_eq!(users["total"], 3);
    assert!(!users.to_string().contains("password_hash"));

    let self_delete = send(
        &app,
        "DELETE",
        "/api/v2/admin/users/admin",
        Some(&admin_token),
        json!({ "confirmation": "DELETE admin" }),
    )
    .await;
    assert_eq!(self_delete.status(), StatusCode::BAD_REQUEST);

    let activity = body(
        send(
            &app,
            "GET",
            "/api/v2/admin/audit",
            Some(&admin_token),
            json!({}),
        )
        .await,
    )
    .await;
    let actions = activity["items"]
        .as_array()
        .expect("audit items")
        .iter()
        .map(|event| event["action"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(actions.contains(&"instance.update"));
    assert!(actions.contains(&"user.create"));
}

#[tokio::test]
async fn integration_reads_redact_secrets_and_deletes_require_exact_confirmation() {
    let (app, services, _dir) = app().await;
    let (_admin, token) = register(&services, "admin", true).await;

    let saved = send(
        &app,
        "POST",
        "/api/v2/admin/remotes",
        Some(&token),
        json!({
            "id": "ice-lab",
            "type": "ice",
            "url": "https://ice.example.org",
            "apiToken": "very-secret-token",
            "nested": { "password": "do-not-return" }
        }),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::OK);

    let integrations = body(
        send(
            &app,
            "GET",
            "/api/v2/admin/integrations",
            Some(&token),
            json!({}),
        )
        .await,
    )
    .await;
    let encoded = integrations.to_string();
    assert!(encoded.contains("[redacted]"));
    assert!(!encoded.contains("very-secret-token"));
    assert!(!encoded.contains("do-not-return"));

    let refused = send(
        &app,
        "DELETE",
        "/api/v2/admin/remotes/ice-lab",
        Some(&token),
        json!({ "confirmation": "delete" }),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);

    let deleted = send(
        &app,
        "DELETE",
        "/api/v2/admin/remotes/ice-lab",
        Some(&token),
        json!({ "confirmation": "DELETE REMOTE ice-lab" }),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn registry_backup_validates_integrity_and_restores_atomically() {
    let (app, services, _dir) = app().await;
    let (admin, token) = register(&services, "admin", true).await;
    services
        .store
        .import_document(ImportInput {
            body: DOCUMENT.to_owned(),
            format: SerializationFormat::Turtle,
            namespace: Some("https://example.org/".to_owned()),
            source_uri: Some("https://source.example/component.ttl".to_owned()),
            document_iri: Some(IriString::new("https://example.org/document").expect("iri")),
            created_by: Some(admin.graph_uri),
            name: Some("Portable design".to_owned()),
            description: Some("Backup roundtrip".to_owned()),
            overwrite: ImportOverwrite::Fail,
        })
        .await
        .expect("import document");

    let exported = send(&app, "GET", "/api/v2/admin/backup", Some(&token), json!({})).await;
    assert_eq!(exported.status(), StatusCode::OK);
    assert_eq!(exported.headers()["cache-control"], "no-store");
    let archive = body(exported).await;
    assert_eq!(archive["format"], "sbol-db-registry-backup");
    assert_eq!(archive["documents"].as_array().unwrap().len(), 1);

    let mut tampered = archive.clone();
    tampered["documents"][0]["name"] = json!("tampered");
    let invalid = send(
        &app,
        "POST",
        "/api/v2/admin/backup/validate",
        Some(&token),
        tampered,
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

    let validation = body(
        send(
            &app,
            "POST",
            "/api/v2/admin/backup/validate",
            Some(&token),
            archive.clone(),
        )
        .await,
    )
    .await;
    assert_eq!(validation["valid"], true);
    let confirmation = validation["confirmation"].as_str().unwrap();

    let restored = send(
        &app,
        "POST",
        "/api/v2/admin/backup/restore",
        Some(&token),
        json!({ "archive": archive, "confirmation": confirmation }),
    )
    .await;
    assert_eq!(restored.status(), StatusCode::OK);
    let restored = body(restored).await;
    assert_eq!(restored["documents"], 1);
    assert_eq!(restored["status"], "restored");
    assert_eq!(restored["rebuild_job"]["kind"], "rebuild_search_index");
}
