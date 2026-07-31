//! Portal bootstrap and browser-session contract tests.
//!
//! These exercise the real combined router over SQLite so the V2 contracts,
//! V1 compatibility endpoints, cookie transport, and shared application facade
//! are proven together rather than as isolated handler units.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::header::{CACHE_CONTROL, SET_COOKIE, VARY};
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use sbol_db_app::{AppServices, Registration};
use sbol_db_backend::Backend;
use sbol_db_core::User;
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

const BODY_LIMIT: usize = 1024 * 1024;

async fn app_with(config: ServerConfig) -> (Router, Arc<AppServices>, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("v2-session.db");
    let url = format!("sqlite://{}", path.display());
    let backend = Backend::open(&url).await.expect("open sqlite backend");
    backend
        .migrator
        .as_ref()
        .expect("sqlite backend has a migrator")
        .run_migrations()
        .await
        .expect("run migrations");

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

async fn register(services: &AppServices, username: &str, email: &str, is_admin: bool) -> User {
    services
        .auth
        .register(Registration {
            username: username.to_owned(),
            name: format!("{username} name"),
            email: email.to_owned(),
            affiliation: Some("Example Lab".to_owned()),
            password: "s3cret".to_owned(),
            is_admin,
            is_curator: is_admin,
            is_member: true,
        })
        .await
        .expect("register user")
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: impl Into<Body>,
) -> Response<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    app.clone()
        .oneshot(builder.body(body.into()).expect("request"))
        .await
        .expect("response")
}

async fn json_body(response: Response<Body>) -> Value {
    let bytes = to_bytes(response.into_body(), BODY_LIMIT)
        .await
        .expect("response body");
    serde_json::from_slice(&bytes).expect("JSON response")
}

fn cookie_pair(response: &Response<Body>) -> String {
    response
        .headers()
        .get(SET_COOKIE)
        .expect("Set-Cookie")
        .to_str()
        .expect("cookie text")
        .split(';')
        .next()
        .expect("cookie pair")
        .to_owned()
}

#[tokio::test]
async fn instance_contract_is_safe_and_matches_enforced_registration_policy() {
    let (app, services, _dir) = app_with(ServerConfig::default()).await;

    let response = send(&app, "GET", "/api/v2/instance", &[], Body::empty()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let initial = json_body(response).await;
    assert_eq!(initial["name"], "SBOL DB");
    assert_eq!(initial["setup_required"], true);
    assert_eq!(initial["policies"]["allow_public_signup"], true);
    assert_eq!(initial["capabilities"]["browser_sessions"], true);

    register(&services, "admin", "admin@example.org", true).await;
    services
        .config
        .set(
            "theme",
            &json!({
                "instanceName": "Cell Atlas",
                "instanceUrl": "https://parts.example.org",
                "uriPrefix": "https://parts.example.org/",
                "frontPageText": "A **curated** registry",
                "themeParameters": [
                    {"name": "Base Color", "variable": "baseColor", "value": "#D25627"}
                ],
                "allowPublicSignup": false,
                "requireLogin": true,
                "firstLaunch": true,
                "sendgridApiKey": "must-never-be-public"
            }),
        )
        .await
        .expect("store theme");

    let response = send(&app, "GET", "/api/v2/instance", &[], Body::empty()).await;
    let instance = json_body(response).await;
    assert_eq!(instance["name"], "Cell Atlas");
    // Classic clients may retain their stored theme, but it is deliberately
    // absent from the native SBOL DB bootstrap and cannot recolor the app.
    assert!(instance.get("accent_color").is_none());
    assert_eq!(instance["setup_required"], false);
    assert_eq!(instance["policies"]["allow_public_signup"], false);
    assert_eq!(instance["policies"]["require_login"], true);
    let encoded = instance.to_string();
    assert!(!encoded.contains("sendgrid"));
    assert!(!encoded.contains("must-never-be-public"));

    // Derived setup state also wins on the compatibility representation.
    let theme = json_body(send(&app, "GET", "/admin/theme", &[], Body::empty()).await).await;
    assert_eq!(theme["firstLaunch"], false);

    // The same stored policy drives actual registration, not just UI display.
    let form = "name=Visitor&username=visitor&email=visitor%40example.org&password=s3cret";
    let response = send(
        &app,
        "POST",
        "/register",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("accept", "application/json"),
        ],
        Body::from(form),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn browser_session_login_resolution_and_logout_share_one_revoked_token() {
    let config = ServerConfig {
        session_cookie_secure: true,
        ..ServerConfig::default()
    };
    let (app, services, _dir) = app_with(config).await;
    let alice = register(&services, "alice", "alice@example.org", false).await;
    let login = json!({"identifier": "alice@example.org", "password": "s3cret"}).to_string();
    let response = send(
        &app,
        "POST",
        "/api/v2/session",
        &[
            ("content-type", "application/json"),
            ("host", "parts.example.org"),
            ("origin", "https://parts.example.org"),
        ],
        Body::from(login),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
    let set_cookie = response.headers()[SET_COOKIE].to_str().unwrap();
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Lax"));
    assert!(set_cookie.contains("Secure"));
    let cookie = cookie_pair(&response);
    let logged_in = json_body(response).await;
    assert_eq!(logged_in["authenticated"], true);
    assert_eq!(logged_in["user"]["id"], alice.id.to_string());
    assert_eq!(logged_in["user"]["username"], "alice");
    assert!(logged_in.get("token").is_none());
    let encoded = logged_in.to_string();
    assert!(!encoded.contains("password"));
    assert!(!encoded.contains("reset_password"));

    let response = send(
        &app,
        "GET",
        "/api/v2/session",
        &[("cookie", &cookie)],
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()[VARY]
        .to_str()
        .unwrap()
        .contains("Authorization"));
    assert_eq!(json_body(response).await["user"]["username"], "alice");

    let response = send(
        &app,
        "DELETE",
        "/api/v2/session",
        &[
            ("cookie", &cookie),
            ("host", "parts.example.org"),
            ("origin", "https://parts.example.org"),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(response.headers()[SET_COOKIE]
        .to_str()
        .unwrap()
        .contains("Max-Age=0"));

    let response = send(
        &app,
        "GET",
        "/api/v2/session",
        &[("cookie", &cookie)],
        Body::empty(),
    )
    .await;
    let logged_out = json_body(response).await;
    assert_eq!(logged_out["authenticated"], false);
    assert!(logged_out["user"].is_null());
}

#[tokio::test]
async fn credential_precedence_and_origin_guards_are_explicit() {
    let (app, services, _dir) = app_with(ServerConfig::default()).await;
    let alice = register(&services, "alice", "alice@example.org", false).await;
    let bob = register(&services, "bob", "bob@example.org", false).await;
    let admin = register(&services, "admin", "admin@example.org", true).await;
    let alice_token = services.auth.issue_token(alice.id).await.unwrap();
    let bob_token = services.auth.issue_token(bob.id).await.unwrap();
    let admin_token = services.auth.issue_token(admin.id).await.unwrap();
    let alice_cookie = format!("sbol-db-token={alice_token}");

    // An explicit bearer credential wins over ambient cookie authority.
    let bearer = format!("Bearer {bob_token}");
    let response = send(
        &app,
        "GET",
        "/api/v2/session",
        &[("authorization", &bearer), ("cookie", &alice_cookie)],
        Body::empty(),
    )
    .await;
    assert_eq!(json_body(response).await["user"]["username"], "bob");

    // A malformed explicit Authorization header does not fall through.
    let response = send(
        &app,
        "GET",
        "/api/v2/session",
        &[
            ("authorization", "Basic not-bearer"),
            ("cookie", &alice_cookie),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(json_body(response).await["authenticated"], false);

    // Cookie-backed unsafe requests are same-origin only, and rejection does
    // not revoke the session.
    let response = send(
        &app,
        "DELETE",
        "/api/v2/session",
        &[
            ("cookie", &alice_cookie),
            ("host", "parts.example.org"),
            ("origin", "https://evil.example"),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(response).await["error"]["code"], "forbidden");
    let response = send(
        &app,
        "GET",
        "/api/v2/session",
        &[("cookie", &alice_cookie)],
        Body::empty(),
    )
    .await;
    assert_eq!(json_body(response).await["user"]["username"], "alice");

    let response = send(
        &app,
        "POST",
        "/api/v2/session",
        &[
            ("content-type", "application/json"),
            ("host", "parts.example.org"),
            ("origin", "https://evil.example"),
        ],
        Body::from(json!({"identifier": "alice", "password": "s3cret"}).to_string()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = send(
        &app,
        "POST",
        "/api/v2/session",
        &[("content-type", "application/json")],
        Body::from(json!({"identifier": "alice", "password": "wrong"}).to_string()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers().get(SET_COOKIE).is_none());
    assert_eq!(json_body(response).await["error"]["code"], "unauthorized");

    // The relocated workbench is protected at its JSON boundary, not just by
    // a client-side route guard.
    let response = send(&app, "GET", "/lab/api/info", &[], Body::empty()).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let alice_bearer = format!("Bearer {alice_token}");
    let response = send(
        &app,
        "GET",
        "/lab/api/info",
        &[("authorization", &alice_bearer)],
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let admin_bearer = format!("Bearer {admin_token}");
    let response = send(
        &app,
        "GET",
        "/lab/api/info",
        &[("authorization", &admin_bearer)],
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn require_login_policy_is_enforced_by_v2_resources() {
    let (app, services, _dir) = app_with(ServerConfig::default()).await;
    let admin = register(&services, "admin", "admin@example.org", true).await;
    services
        .config
        .set("theme", &json!({"requireLogin": true}))
        .await
        .expect("store require-login policy");

    for path in ["/api/v2", "/api/v2/instance", "/api/v2/session"] {
        let response = send(&app, "GET", path, &[], Body::empty()).await;
        assert_eq!(response.status(), StatusCode::OK, "bootstrap path {path}");
    }

    let response = send(&app, "GET", "/api/v2/search", &[], Body::empty()).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json_body(response).await["error"]["code"], "unauthorized");

    let token = services.auth.issue_token(admin.id).await.unwrap();
    let bearer = format!("Bearer {token}");
    let response = send(
        &app,
        "GET",
        "/api/v2/search",
        &[("authorization", &bearer)],
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn compatibility_browser_login_cookie_is_understood_and_revoked_by_v2() {
    let (app, services, _dir) = app_with(ServerConfig::default()).await;
    register(&services, "alice", "alice@example.org", false).await;
    let response = send(
        &app,
        "POST",
        "/login",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("accept", "text/html"),
        ],
        Body::from("email=alice%40example.org&password=s3cret"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FOUND);
    let cookie = cookie_pair(&response);

    let response = send(
        &app,
        "GET",
        "/api/v2/session",
        &[("cookie", &cookie)],
        Body::empty(),
    )
    .await;
    assert_eq!(json_body(response).await["user"]["username"], "alice");

    // V1 logout accepts the browser cookie for revocation even though V1 data
    // handlers retain their X-authorization compatibility contract.
    let response = send(
        &app,
        "POST",
        "/logout",
        &[("accept", "text/html"), ("cookie", &cookie)],
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert!(response.headers()[SET_COOKIE]
        .to_str()
        .unwrap()
        .contains("Max-Age=0"));

    let response = send(
        &app,
        "GET",
        "/api/v2/session",
        &[("cookie", &cookie)],
        Body::empty(),
    )
    .await;
    assert_eq!(json_body(response).await["authenticated"], false);
}
