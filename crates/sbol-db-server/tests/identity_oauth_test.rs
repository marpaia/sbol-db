//! End-to-end SBOL Identity OAuth discovery, browser consent, PKCE exchange,
//! refresh rotation, revocation, and MCP resource binding.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::header::{ACCESS_CONTROL_ALLOW_ORIGIN, CONTENT_TYPE, COOKIE, LOCATION, SET_COOKIE};
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ring::signature::{UnparsedPublicKey, ED25519};
use sbol_db_app::{AppServices, Registration};
use sbol_db_backend::Backend;
use sbol_db_server::{router, AppState, Metrics, SchemaCache, ServerConfig};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tower::ServiceExt;
use url::Url;

const ORIGIN: &str = "http://127.0.0.1:8888";
const RESOURCE: &str = "http://127.0.0.1:8888/mcp";
const REDIRECT_URI: &str = "http://127.0.0.1:43123/callback";
const BODY_LIMIT: usize = 4 * 1024 * 1024;
const VERIFIER: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";
const DESIGN: &str = r#"
@prefix sbol: <http://sbols.org/v2#> .
<http://example.org/api_part/1>
    a sbol:ComponentDefinition ;
    sbol:displayId "api_part" ;
    sbol:persistentIdentity <http://example.org/api_part> ;
    sbol:version "1" ;
    sbol:type <http://www.biopax.org/release/biopax-level3.owl#DnaRegion> .
"#;

async fn app() -> (Router, Arc<AppServices>, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("identity.db");
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
        public_origin: Some(ORIGIN.to_owned()),
        mcp_enabled: true,
        ..ServerConfig::default()
    };
    config.resolve_public_origin(ORIGIN).unwrap();
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

async fn body_json(response: Response<Body>) -> Value {
    let bytes = to_bytes(response.into_body(), BODY_LIMIT)
        .await
        .expect("body bytes");
    serde_json::from_slice(&bytes).expect("JSON body")
}

async fn register_client(app: &Router) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/register")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "client_name": "Biodesign Agent",
                        "redirect_uris": [REDIRECT_URI],
                        "grant_types": ["authorization_code", "refresh_token"],
                        "response_types": ["code"],
                        "token_endpoint_auth_method": "none"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    body_json(response).await
}

async fn login_cookie(app: &Router) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v2/session")
                .header("host", "127.0.0.1:8888")
                .header("origin", ORIGIN)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "identifier": "alice", "password": "s3cret" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.headers()[SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

fn authorization_url(client_id: &str, scopes: &str) -> String {
    authorization_url_for_resource(client_id, scopes, RESOURCE)
}

fn authorization_url_for_resource(client_id: &str, scopes: &str, resource: &str) -> String {
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()));
    let mut url = Url::parse(&format!("{ORIGIN}/oauth/authorize")).unwrap();
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("resource", resource)
        .append_pair("scope", scopes)
        .append_pair("state", "state-123");
    format!("{}?{}", url.path(), url.query().unwrap())
}

fn openid_authorization_url(client_id: &str, nonce: &str) -> String {
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()));
    let mut url = Url::parse(&format!("{ORIGIN}/oauth/authorize")).unwrap();
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("scope", "openid profile email")
        .append_pair("state", "state-123")
        .append_pair("nonce", nonce);
    format!("{}?{}", url.path(), url.query().unwrap())
}

async fn consent_code(app: &Router, client_id: &str, cookie: &str, scopes: &str) -> String {
    let uri = authorization_url(client_id, scopes);
    consent_code_for_uri(app, cookie, &uri).await
}

async fn consent_code_for_uri(app: &Router, cookie: &str, uri: &str) -> String {
    let page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(COOKIE, cookie)
                .header("accept", "text/html")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let html = String::from_utf8(
        to_bytes(page.into_body(), BODY_LIMIT)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("Biodesign Agent"));
    assert!(html.contains("SBOL Identity"));

    let query: Vec<(String, String)> = Url::parse(&format!("{ORIGIN}{uri}"))
        .unwrap()
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .chain([("decision".to_owned(), "allow".to_owned())])
        .collect();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/authorize")
                .header("host", "127.0.0.1:8888")
                .header("origin", ORIGIN)
                .header(COOKIE, cookie)
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(serde_urlencoded::to_string(query).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    let location = Url::parse(response.headers()[LOCATION].to_str().unwrap()).unwrap();
    assert_eq!(
        location
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1,
        "state-123"
    );
    location
        .query_pairs()
        .find(|(key, _)| key == "code")
        .unwrap()
        .1
        .into_owned()
}

async fn exchange_code(app: &Router, client_id: &str, code: &str) -> Value {
    exchange_code_for_resource(app, client_id, code, RESOURCE).await
}

async fn exchange_code_for_resource(
    app: &Router,
    client_id: &str,
    code: &str,
    resource: &str,
) -> Value {
    let form = serde_urlencoded::to_string([
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", client_id),
        ("redirect_uri", REDIRECT_URI),
        ("resource", resource),
        ("code_verifier", VERIFIER),
    ])
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

async fn exchange_openid_code(app: &Router, client_id: &str, code: &str) -> Value {
    let form = serde_urlencoded::to_string([
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", client_id),
        ("redirect_uri", REDIRECT_URI),
        ("code_verifier", VERIFIER),
    ])
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

async fn oauth_access_for_resource(
    app: &Router,
    client_id: &str,
    cookie: &str,
    scopes: &str,
    resource: &str,
) -> Value {
    let uri = authorization_url_for_resource(client_id, scopes, resource);
    let code = consent_code_for_uri(app, cookie, &uri).await;
    exchange_code_for_resource(app, client_id, &code, resource).await
}

#[tokio::test]
async fn identity_discovery_and_dynamic_registration_are_mcp_compatible() {
    let (app, _services, _dir) = app().await;
    let resource = body_json(
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-protected-resource/mcp")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(resource["resource"], RESOURCE);
    assert_eq!(resource["authorization_servers"][0], ORIGIN);
    assert_eq!(
        resource["scopes_supported"],
        json!(["sbol:read", "sbol:write", "sbol:share", "sbol:review"])
    );

    let api_resource = body_json(
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-protected-resource/api/v2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(api_resource["resource"], format!("{ORIGIN}/api/v2"));
    assert_eq!(api_resource["authorization_servers"][0], ORIGIN);

    let metadata = body_json(
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-authorization-server")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(metadata["issuer"], ORIGIN);
    assert_eq!(
        metadata["authorization_endpoint"],
        format!("{ORIGIN}/oauth/authorize")
    );
    assert_eq!(
        metadata["code_challenge_methods_supported"],
        json!(["S256"])
    );
    assert!(metadata["scopes_supported"]
        .as_array()
        .unwrap()
        .contains(&json!("sbol:share")));

    let client = register_client(&app).await;
    assert!(client["client_id"]
        .as_str()
        .unwrap()
        .starts_with("sbol_client_"));
    assert_eq!(client["token_endpoint_auth_method"], "none");
}

#[tokio::test]
async fn identity_metadata_and_token_endpoints_support_public_browser_clients() {
    let (app, _services, _dir) = app().await;
    let origin = "https://canvas.example";

    let metadata = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/.well-known/openid-configuration")
                .header("origin", origin)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(metadata.status(), StatusCode::OK);
    assert_eq!(metadata.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");

    let preflight = app
        .clone()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/oauth/token")
                .header("origin", origin)
                .header("access-control-request-method", "POST")
                .header("access-control-request-headers", "content-type")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(preflight.status(), StatusCode::OK);
    assert_eq!(preflight.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
}

#[tokio::test]
async fn browser_consent_pkce_refresh_and_revocation_authorize_mcp() {
    let (app, services, _dir) = app().await;
    services
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
    let client = register_client(&app).await;
    let client_id = client["client_id"].as_str().unwrap();

    let anonymous = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(authorization_url(client_id, "sbol:read"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::FOUND);
    assert!(anonymous.headers()[LOCATION]
        .to_str()
        .unwrap()
        .starts_with("/login?next="));

    let cookie = login_cookie(&app).await;
    let code = consent_code(&app, client_id, &cookie, "sbol:read").await;
    let token = exchange_code(&app, client_id, &code).await;
    assert_eq!(token["token_type"], "Bearer");
    assert_eq!(token["scope"], "sbol:read");
    assert_eq!(token["resource"], RESOURCE);

    let access = token["access_token"].as_str().unwrap();
    let initialize = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(CONTENT_TYPE, "application/json")
                .header("accept", "application/json, text/event-stream")
                .header("authorization", format!("Bearer {access}"))
                .body(Body::from(
                    json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "initialize",
                        "params": { "protocolVersion": "2025-11-25" }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(initialize.status(), StatusCode::OK);

    let refresh = token["refresh_token"].as_str().unwrap();
    let refresh_form = serde_urlencoded::to_string([
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh),
        ("client_id", client_id),
        ("resource", RESOURCE),
    ])
    .unwrap();
    let rotated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(refresh_form.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rotated.status(), StatusCode::OK);
    let rotated = body_json(rotated).await;
    assert_ne!(rotated["refresh_token"], token["refresh_token"]);

    let replay = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(refresh_form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(replay).await["error"], "invalid_grant");

    let rotated_access = rotated["access_token"].as_str().unwrap();
    let revoke_form = serde_urlencoded::to_string([("token", rotated_access)]).unwrap();
    let revoked = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/revoke")
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(revoke_form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::OK);

    let rejected = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(CONTENT_TYPE, "application/json")
                .header("accept", "application/json, text/event-stream")
                .header("authorization", format!("Bearer {rotated_access}"))
                .body(Body::from(
                    json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn delegated_api_tokens_enforce_read_and_write_scopes() {
    let (app, services, _dir) = app().await;
    services
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
    let client = register_client(&app).await;
    let client_id = client["client_id"].as_str().unwrap();
    let cookie = login_cookie(&app).await;
    let api_resource = format!("{ORIGIN}/api/v2");
    let request = json!({
        "id": "oauth_api",
        "version": "1",
        "name": "OAuth API design",
        "format": "turtle",
        "content": DESIGN,
        "overwrite": "fail"
    });

    let read =
        oauth_access_for_resource(&app, client_id, &cookie, "sbol:read", &api_resource).await;
    let read_access = read["access_token"].as_str().unwrap();

    // A delegated bearer is not the resource-owner browser session and cannot
    // be used to approve a second or broader grant.
    let bearer_only_authorize = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(authorization_url(client_id, "sbol:read sbol:write"))
                .header("authorization", format!("Bearer {read_access}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bearer_only_authorize.status(), StatusCode::FOUND);
    assert!(bearer_only_authorize.headers()[LOCATION]
        .to_str()
        .unwrap()
        .starts_with("/login?next="));

    let preview = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v2/collections/validate")
                .header(CONTENT_TYPE, "application/json")
                .header("authorization", format!("Bearer {read_access}"))
                .body(Body::from(request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let preview_status = preview.status();
    let preview_body = body_json(preview).await;
    assert_eq!(
        preview_status,
        StatusCode::OK,
        "preview body: {preview_body}"
    );

    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v2/collections")
                .header(CONTENT_TYPE, "application/json")
                .header("authorization", format!("Bearer {read_access}"))
                .body(Body::from(request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let challenge = denied.headers()["www-authenticate"].to_str().unwrap();
    assert!(challenge.contains("insufficient_scope"));
    assert!(challenge.contains("sbol:read sbol:write"));
    assert!(challenge.contains("oauth-protected-resource/api/v2"));

    let write = oauth_access_for_resource(
        &app,
        client_id,
        &cookie,
        "sbol:read sbol:write",
        &api_resource,
    )
    .await;
    let write_access = write["access_token"].as_str().unwrap();
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v2/collections")
                .header(CONTENT_TYPE, "application/json")
                .header("authorization", format!("Bearer {write_access}"))
                .body(Body::from(request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn sign_in_with_sbol_issues_verifiable_id_token_and_scoped_userinfo() {
    let (app, services, _dir) = app().await;
    let user = services
        .auth
        .register(Registration {
            username: "alice".to_owned(),
            name: "Alice Example".to_owned(),
            email: "alice@example.org".to_owned(),
            affiliation: Some("Example University".to_owned()),
            password: "s3cret".to_owned(),
            is_admin: false,
            is_curator: false,
            is_member: true,
        })
        .await
        .unwrap();
    let client = register_client(&app).await;
    let client_id = client["client_id"].as_str().unwrap();
    let cookie = login_cookie(&app).await;
    let nonce = "nonce-from-client-123";
    let uri = openid_authorization_url(client_id, nonce);
    let code = consent_code_for_uri(&app, &cookie, &uri).await;
    let token = exchange_openid_code(&app, client_id, &code).await;

    assert_eq!(token["resource"], format!("{ORIGIN}/oauth/userinfo"));
    assert_eq!(token["scope"], "email openid profile");
    let id_token = token["id_token"].as_str().expect("signed ID token");
    let pieces = id_token.split('.').collect::<Vec<_>>();
    assert_eq!(pieces.len(), 3);
    let header: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(pieces[0]).unwrap()).unwrap();
    let claims: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(pieces[1]).unwrap()).unwrap();

    let configuration = body_json(
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/.well-known/openid-configuration")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(configuration["issuer"], ORIGIN);
    assert_eq!(
        configuration["id_token_signing_alg_values_supported"],
        json!(["EdDSA"])
    );
    assert_eq!(
        configuration["userinfo_endpoint"],
        format!("{ORIGIN}/oauth/userinfo")
    );

    let jwks = body_json(
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/oauth/jwks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let jwk = &jwks["keys"][0];
    assert_eq!(header["alg"], "EdDSA");
    assert_eq!(header["kid"], jwk["kid"]);
    let public_key = URL_SAFE_NO_PAD.decode(jwk["x"].as_str().unwrap()).unwrap();
    let signature = URL_SAFE_NO_PAD.decode(pieces[2]).unwrap();
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(
            format!("{}.{}", pieces[0], pieces[1]).as_bytes(),
            &signature,
        )
        .expect("JWKS key verifies the ID token");

    assert_eq!(claims["iss"], ORIGIN);
    assert_eq!(claims["aud"], client_id);
    assert_eq!(claims["sub"], user.id.to_string());
    assert_eq!(claims["nonce"], nonce);
    assert_eq!(claims["preferred_username"], "alice");
    assert_eq!(claims["name"], "Alice Example");
    assert_eq!(claims["email"], "alice@example.org");
    assert_eq!(claims["affiliation"], "Example University");
    assert!(claims["exp"].as_i64().unwrap() > claims["iat"].as_i64().unwrap());

    let access = token["access_token"].as_str().unwrap();
    let userinfo = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/oauth/userinfo")
                .header("authorization", format!("Bearer {access}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(userinfo.status(), StatusCode::OK);
    let userinfo = body_json(userinfo).await;
    assert_eq!(userinfo["sub"], user.id.to_string());
    assert_eq!(userinfo["preferred_username"], "alice");
    assert_eq!(userinfo["email"], "alice@example.org");
    assert!(userinfo.get("is_admin").is_none());

    // An identity token is not an MCP token: exact resource binding prevents
    // a client from replaying it against the design database.
    let mcp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(CONTENT_TYPE, "application/json")
                .header("accept", "application/json, text/event-stream")
                .header("authorization", format!("Bearer {access}"))
                .body(Body::from(
                    json!({ "jsonrpc": "2.0", "id": 9, "method": "ping" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(mcp.status(), StatusCode::UNAUTHORIZED);

    // The same UserInfo token is also rejected by the V2 protected resource,
    // instead of being silently downgraded to anonymous public access.
    let api = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v2/objects?limit=1")
                .header("authorization", format!("Bearer {access}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(api.status(), StatusCode::UNAUTHORIZED);
    let challenge = api.headers()["www-authenticate"].to_str().unwrap();
    assert!(challenge.contains("invalid_token"));
    assert!(challenge.contains("oauth-protected-resource/api/v2"));
}
