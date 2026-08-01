//! Native administrator boundary and integrity workflow tests.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use sbol_db_app::{AppServices, Registration};
use sbol_db_backend::Backend;
use sbol_db_core::User;
use sbol_db_server::{
    router, write_edge_settings, AppState, EdgeAdminService, EdgeRuntimeIdentity, EdgeSettings,
    Metrics, SchemaCache, ServerConfig,
};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 4 * 1024 * 1024;
async fn app() -> (Router, Arc<AppServices>, TempDir) {
    app_with_backups(false).await
}

async fn app_with_backups(complete_backups_enabled: bool) -> (Router, Arc<AppServices>, TempDir) {
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
    let config = ServerConfig {
        complete_backups_enabled,
        ..ServerConfig::default()
    };
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

async fn app_with_edge() -> (Router, Arc<AppServices>, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("v2-edge-admin.db");
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
    let metrics = Metrics::install(None, env!("CARGO_PKG_VERSION"))
        .with_data_disk(dir.path().to_path_buf(), 1);
    metrics.require_tls();
    let settings = EdgeSettings {
        version: 1,
        hostname: "registry.example.org".to_owned(),
        acme_contact: "admin@example.org".to_owned(),
        acme_directory_url: "https://acme.example.org/directory".to_owned(),
        http_redirect_enabled: true,
        tls_handshake_timeout_secs: 10,
        backup_recovery_recipient: age::x25519::Identity::generate().to_public().to_string(),
        backup_repository_url: "s3://backups/registry/production".to_owned(),
        backup_interval_secs: 86_400,
        backup_local_retention: 2,
        minimum_free_bytes: 2_147_483_648,
    }
    .validate()
    .expect("edge settings");
    write_edge_settings(backend.config.as_ref(), &settings)
        .await
        .expect("persist edge settings");
    let mut config = ServerConfig {
        complete_backups_enabled: true,
        ..ServerConfig::default()
    };
    config.edge_admin = Some(Arc::new(EdgeAdminService::new(
        backend.config.clone(),
        settings,
        EdgeRuntimeIdentity {
            profile: "production",
            layout_version: "2".to_owned(),
            generation: uuid::Uuid::new_v4(),
            data_dir: dir.path().display().to_string(),
        },
        metrics.clone(),
    )));
    let state = AppState {
        service: backend.store.clone(),
        sparql: Arc::new(SparqlEngine::new(backend.triple_source.clone())),
        sparql_update: Arc::new(SparqlUpdateEngine::new(
            backend.triple_source.clone(),
            backend.triple_writer.clone(),
        )),
        app: services.clone(),
        metrics,
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
async fn all_backup_triggers_enqueue_the_one_complete_backup_job() {
    let (app, services, _dir) = app_with_backups(true).await;
    let (_admin, token) = register(&services, "admin", true).await;

    let status = send(&app, "GET", "/api/v2/admin/backup", Some(&token), json!({})).await;
    assert_eq!(status.status(), StatusCode::OK);
    let status = body(status).await;
    assert_eq!(status["enabled"], true);
    assert_eq!(status["strategy"], "complete_encrypted_checkpoint");
    assert_eq!(
        status["components"],
        json!(["rocksdb", "blobs", "search", "acme"])
    );

    let manual = send(
        &app,
        "POST",
        "/api/v2/admin/backup",
        Some(&token),
        json!({}),
    )
    .await;
    assert_eq!(manual.status(), StatusCode::ACCEPTED);
    let manual = body(manual).await;
    assert_eq!(manual["job"]["kind"], "complete_backup");
    assert_eq!(manual["job"]["payload"]["trigger"], "manual");
    assert_eq!(manual["job"]["payload"]["requested_by"], "admin");

    let missing_key = send(
        &app,
        "POST",
        "/api/v2/admin/backup",
        Some(&token),
        json!({ "trigger": "pre_deploy" }),
    )
    .await;
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);

    let first = body(
        send(
            &app,
            "POST",
            "/api/v2/admin/backup",
            Some(&token),
            json!({
                "trigger": "pre_deploy",
                "idempotency_key": "release-abc"
            }),
        )
        .await,
    )
    .await;
    let second = body(
        send(
            &app,
            "POST",
            "/api/v2/admin/backup",
            Some(&token),
            json!({
                "trigger": "pre_deploy",
                "idempotency_key": "release-abc"
            }),
        )
        .await,
    )
    .await;
    assert_eq!(first["job"]["id"], second["job"]["id"]);
    assert_eq!(first["deduplicated"], false);
    assert_eq!(second["deduplicated"], true);

    let old_validate = send(
        &app,
        "POST",
        "/api/v2/admin/backup/validate",
        Some(&token),
        json!({}),
    )
    .await;
    assert_eq!(old_validate.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn complete_backup_trigger_fails_closed_when_runtime_is_absent() {
    let (app, services, _dir) = app().await;
    let (_admin, token) = register(&services, "admin", true).await;
    let response = send(
        &app,
        "POST",
        "/api/v2/admin/backup",
        Some(&token),
        json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn edge_settings_are_validated_persisted_and_report_restart_state() {
    let (app, services, _dir) = app_with_edge().await;
    let (_admin, token) = register(&services, "edge-admin", true).await;

    let initial = send(&app, "GET", "/api/v2/admin/edge", Some(&token), json!({})).await;
    assert_eq!(initial.status(), StatusCode::OK);
    let initial = body(initial).await;
    assert_eq!(initial["restart_required"], false);
    assert_eq!(initial["active"]["hostname"], "registry.example.org");
    assert_eq!(initial["health"]["tls"]["required"], true);
    assert_eq!(initial["health"]["disk"]["ready"], true);

    let updated = send(
        &app,
        "PATCH",
        "/api/v2/admin/edge",
        Some(&token),
        json!({
            "hostname": "next.example.org",
            "backup_interval_secs": 3600,
            "backup_local_retention": 4
        }),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let updated = body(updated).await;
    assert_eq!(updated["restart_required"], true);
    assert_eq!(updated["active"]["hostname"], "registry.example.org");
    assert_eq!(updated["pending"]["hostname"], "next.example.org");
    assert_eq!(updated["pending"]["backup_interval_secs"], 3600);

    let persisted =
        body(send(&app, "GET", "/api/v2/admin/edge", Some(&token), json!({})).await).await;
    assert_eq!(persisted["pending"]["backup_local_retention"], 4);

    let invalid = send(
        &app,
        "PATCH",
        "/api/v2/admin/edge",
        Some(&token),
        json!({ "backup_repository_url": "https://example.org/archive" }),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
}
