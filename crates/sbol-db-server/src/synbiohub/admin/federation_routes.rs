//! Web of Registries federation admin routes plus the public update webhook.
//!
//! The admin routes (`/admin/federate`, `/admin/retrieveFromWebOfRegistries`,
//! `/admin/registries` + save/delete, `/admin/remotes` + save/delete) run behind
//! the admin gate and drive the app-layer
//! [`FederationService`](sbol_db_app::FederationService). Each mirrors classic
//! SynBioHub's `lib/actions/admin/*` action, returning the same `text/plain`
//! status bodies and status codes (`400` on a missing field, `404` on an unknown
//! entry, `503` when the Web of Registries is unreachable).
//!
//! The public [`update_web_of_registries`] webhook is the callback the Web of
//! Registries itself invokes; it is gated on the shared update secret rather
//! than the admin cookie, and enqueues the `wor_sync` job so the sync runs off
//! the request path.

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use sbol_db_app::FederationError;
use sbol_db_storage::{EnqueueOutcome, NewJob};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{parse_body, parse_config_value, CurrentUser};
use crate::AppState;

/// The `wor_sync` job kind the update webhook enqueues.
const WOR_SYNC_KIND: &str = "wor_sync";

/// Whether the resolved caller is an administrator.
fn is_admin(user: &Option<sbol_db_core::User>) -> bool {
    user.as_ref().map(|u| u.is_admin).unwrap_or(false)
}

/// A `text/plain` response with an explicit status, the shape every classic
/// federation action returns.
fn text(status: StatusCode, body: impl Into<String>) -> Response {
    (status, [(CONTENT_TYPE, "text/plain")], body.into()).into_response()
}

/// Map a [`FederationError`] to the classic status code and `text/plain` body.
fn fed_error(err: FederationError) -> Response {
    let (status, message) = match err {
        FederationError::NotAuthorized => (StatusCode::FORBIDDEN, err.to_string()),
        FederationError::MissingField(m) => (StatusCode::BAD_REQUEST, m),
        FederationError::NotFound(m) => (StatusCode::NOT_FOUND, m),
        FederationError::RemoteContact(m) => (StatusCode::SERVICE_UNAVAILABLE, m),
        FederationError::Domain(d) => (StatusCode::INTERNAL_SERVER_ERROR, d.to_string()),
    };
    text(status, message)
}

/// The `POST /admin/federate` body: the administrator email and the Web of
/// Registries URL to join.
#[derive(Debug, Default, Deserialize)]
struct FederateForm {
    #[serde(rename = "administratorEmail", default)]
    administrator_email: String,
    #[serde(rename = "webOfRegistries", default)]
    web_of_registries: String,
}

/// `POST /admin/federate` — request membership in a Web of Registries.
pub async fn federate(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let form: FederateForm = match parse_body(&headers, &body) {
        Ok(f) => f,
        Err(e) => return text(StatusCode::BAD_REQUEST, e.to_string()),
    };
    match state
        .app
        .federation()
        .federate(
            is_admin(&user),
            &form.administrator_email,
            &form.web_of_registries,
        )
        .await
    {
        Ok(()) => text(
            StatusCode::OK,
            "Submitted request to join Web-of-Registries",
        ),
        Err(e) => fed_error(e),
    }
}

/// `POST /admin/retrieveFromWebOfRegistries` — synchronously pull the instance
/// list and refresh the prefix map.
pub async fn retrieve(State(state): State<AppState>) -> Response {
    match state.app.federation().retrieve().await {
        Ok(count) => text(StatusCode::OK, format!("Registries Updated ({count})")),
        Err(e) => fed_error(e),
    }
}

/// `GET /admin/registries` — the current `uriPrefix -> instanceUrl` map.
pub async fn registries(State(state): State<AppState>) -> Response {
    match state.app.federation().registries().await {
        Ok(pairs) => {
            let map: serde_json::Map<String, Value> = pairs
                .into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect();
            Json(Value::Object(map)).into_response()
        }
        Err(e) => fed_error(FederationError::Domain(e)),
    }
}

/// The `POST /admin/saveRegistry` body.
#[derive(Debug, Default, Deserialize)]
struct RegistryForm {
    #[serde(default)]
    uri: String,
    #[serde(default)]
    url: String,
}

/// `POST /admin/saveRegistry` — upsert one `uri -> url` registry entry.
pub async fn save_registry(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let form: RegistryForm = match parse_body(&headers, &body) {
        Ok(f) => f,
        Err(e) => return text(StatusCode::BAD_REQUEST, e.to_string()),
    };
    match state
        .app
        .federation()
        .save_registry(is_admin(&user), &form.uri, &form.url)
        .await
    {
        Ok(()) => text(
            StatusCode::OK,
            format!("Registry ({}, {}) saved successfully", form.uri, form.url),
        ),
        Err(e) => fed_error(e),
    }
}

/// The `POST /admin/deleteRegistry` body.
#[derive(Debug, Default, Deserialize)]
struct DeleteRegistryForm {
    #[serde(default)]
    uri: String,
}

/// `POST /admin/deleteRegistry` — remove a registry entry by URI.
pub async fn delete_registry(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let form: DeleteRegistryForm = match parse_body(&headers, &body) {
        Ok(f) => f,
        Err(e) => return text(StatusCode::BAD_REQUEST, e.to_string()),
    };
    match state
        .app
        .federation()
        .delete_registry(is_admin(&user), &form.uri)
        .await
    {
        Ok(()) => text(
            StatusCode::OK,
            format!("Registry ({}) deleted successfully", form.uri),
        ),
        Err(e) => fed_error(e),
    }
}

/// `GET /admin/remotes` — the configured ICE / Benchling remotes.
pub async fn remotes(State(state): State<AppState>) -> Response {
    match state.app.federation().remotes().await {
        Ok(map) => Json(Value::Object(map)).into_response(),
        Err(e) => fed_error(FederationError::Domain(e)),
    }
}

/// `POST /admin/saveRemote` — upsert a remote config keyed by its id. The body
/// is taken verbatim (JSON) or assembled from form fields, then validated.
pub async fn save_remote(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let value = match parse_config_value(&headers, &body) {
        Ok(v) => v,
        Err(e) => return text(StatusCode::BAD_REQUEST, e.to_string()),
    };
    match state
        .app
        .federation()
        .save_remote(is_admin(&user), value)
        .await
    {
        Ok(id) => text(StatusCode::OK, format!("Remote ({id}) saved successfully")),
        Err(e) => fed_error(e),
    }
}

/// The `POST /admin/deleteRemote` body.
#[derive(Debug, Default, Deserialize)]
struct DeleteRemoteForm {
    #[serde(default)]
    id: String,
}

/// `POST /admin/deleteRemote` — remove a remote by id.
pub async fn delete_remote(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let form: DeleteRemoteForm = match parse_body(&headers, &body) {
        Ok(f) => f,
        Err(e) => return text(StatusCode::BAD_REQUEST, e.to_string()),
    };
    match state
        .app
        .federation()
        .delete_remote(is_admin(&user), &form.id)
        .await
    {
        Ok(()) => text(
            StatusCode::OK,
            format!("Remote ({}) deleted successfully", form.id),
        ),
        Err(e) => fed_error(e),
    }
}

/// The secret the update webhook presents, in the query string.
#[derive(Debug, Default, Deserialize)]
pub struct WebhookQuery {
    #[serde(default)]
    secret: String,
}

/// `POST /updateWebOfRegistries` — the public Web of Registries callback.
///
/// Not admin-gated: the Web of Registries authenticates with the shared update
/// secret returned by the join, presented as the `secret` query parameter and
/// matched against the stored `webOfRegistriesSecret`. A missing, unconfigured,
/// or mismatched secret is `403`. On success it enqueues the `wor_sync` job and
/// returns `200`, so the WoR callback does not block on the pull.
pub async fn update_web_of_registries(
    State(state): State<AppState>,
    Query(query): Query<WebhookQuery>,
) -> Response {
    let expected = match state.app.federation().update_secret().await {
        Ok(secret) => secret,
        Err(e) => return fed_error(FederationError::Domain(e)),
    };
    let authorized = expected
        .as_deref()
        .map(|s| !s.is_empty() && s == query.secret)
        .unwrap_or(false);
    if !authorized {
        return text(StatusCode::FORBIDDEN, "invalid or missing update secret");
    }

    let job = NewJob {
        kind: WOR_SYNC_KIND.to_owned(),
        payload: json!({}),
        queue: None,
        priority: None,
        max_attempts: None,
        idempotency_key: None,
        available_at: None,
        parent_job_id: None,
        correlation_id: None,
    };
    match state.jobs.enqueue(job).await {
        Ok(EnqueueOutcome::Inserted(_)) | Ok(EnqueueOutcome::AlreadyExists(_)) => {
            text(StatusCode::OK, "Registries Updated")
        }
        Err(err) => text(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("enqueue wor_sync job: {err}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use sbol_db_app::{
        AppServices, FederationError, JoinPayload, JoinResponse, Registration,
        WebOfRegistriesClient, WorInstance,
    };
    use sbol_db_backend::Backend;
    use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
    use serde_json::{json, Value};
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::{synbiohub, AppState, Metrics, SchemaCache, ServerConfig};

    /// A stubbed Web of Registries: canned join credentials and a fixed instance
    /// list, so the routes are exercised with no network.
    struct StubClient;

    #[async_trait]
    impl WebOfRegistriesClient for StubClient {
        async fn join(
            &self,
            _wor_url: &str,
            _payload: &JoinPayload,
        ) -> Result<JoinResponse, FederationError> {
            Ok(JoinResponse {
                id: "inst-1".to_owned(),
                update_secret: "top-secret".to_owned(),
            })
        }

        async fn fetch_instances(
            &self,
            _wor_url: &str,
        ) -> Result<Vec<WorInstance>, FederationError> {
            Ok(vec![
                WorInstance {
                    uri_prefix: "https://a.org/".to_owned(),
                    instance_url: "https://a.org/".to_owned(),
                },
                WorInstance {
                    uri_prefix: "https://b.org/".to_owned(),
                    instance_url: "https://b.org".to_owned(),
                },
            ])
        }

        async fn fetch_sbol(&self, _object_url: &str) -> Result<String, FederationError> {
            Ok(String::new())
        }
    }

    struct Fixture {
        app: Router,
        admin_token: String,
        _dir: TempDir,
    }

    async fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("federation.db");
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
        let app_services = Arc::new(
            AppServices::from_backend(&backend).with_federation_client(Arc::new(StubClient)),
        );

        let admin = app_services
            .auth
            .register(Registration {
                username: "root".to_owned(),
                name: "Root".to_owned(),
                email: "root@example.org".to_owned(),
                affiliation: None,
                password: "s3cret".to_owned(),
                is_admin: true,
                is_curator: false,
                is_member: true,
            })
            .await
            .expect("register admin");
        let admin_token = app_services
            .auth
            .issue_token(admin.id)
            .await
            .expect("issue admin token");

        let state = AppState {
            service: backend.store.clone(),
            sparql,
            sparql_update,
            app: app_services,
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
        let app = synbiohub::router(state.clone()).with_state(state);
        Fixture {
            app,
            admin_token,
            _dir: dir,
        }
    }

    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        token: Option<&str>,
        content_type: Option<&str>,
        body: Body,
    ) -> (StatusCode, String) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header("x-authorization", token);
        }
        if let Some(ct) = content_type {
            builder = builder.header("content-type", ct);
        }
        let res = app
            .clone()
            .oneshot(builder.body(body).unwrap())
            .await
            .expect("request");
        let status = res.status();
        let bytes = to_bytes(res.into_body(), 1024 * 1024).await.expect("body");
        (status, String::from_utf8(bytes.to_vec()).expect("utf8"))
    }

    async fn json_post(app: &Router, uri: &str, token: &str, body: Value) -> (StatusCode, String) {
        send(
            app,
            "POST",
            uri,
            Some(token),
            Some("application/json"),
            Body::from(body.to_string()),
        )
        .await
    }

    /// Joining then syncing against the stub stores the returned credentials and
    /// the expected `uriPrefix -> instanceUrl` map, readable at
    /// `/admin/registries`.
    #[tokio::test]
    async fn federate_then_sync_populates_the_registry_map() {
        let fx = fixture().await;

        // Seed the instance identity the join payload advertises.
        let (status, _) = json_post(
            &fx.app,
            "/admin/mail",
            &fx.admin_token,
            json!({ "fromAddress": "ops@example.org" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Join.
        let (status, body) = json_post(
            &fx.app,
            "/admin/federate",
            &fx.admin_token,
            json!({
                "administratorEmail": "admin@example.org",
                "webOfRegistries": "https://wor.example.org/",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "federate body: {body}");

        // Sync.
        let (status, _) = json_post(
            &fx.app,
            "/admin/retrieveFromWebOfRegistries",
            &fx.admin_token,
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // The map holds the stub's instances, trailing slash stripped.
        let (status, body) = send(
            &fx.app,
            "GET",
            "/admin/registries",
            Some(&fx.admin_token),
            None,
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let map: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(map["https://a.org/"], "https://a.org");
        assert_eq!(map["https://b.org/"], "https://b.org");
    }

    /// The public webhook is gated on the stored update secret: wrong or missing
    /// secret is `403`; the matching secret enqueues the sync and returns `200`.
    #[tokio::test]
    async fn update_webhook_is_secret_gated() {
        let fx = fixture().await;

        // No secret configured yet: even a caller-supplied secret is rejected.
        let (status, _) = send(
            &fx.app,
            "POST",
            "/updateWebOfRegistries?secret=whatever",
            None,
            None,
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // Federate to establish the shared secret ("top-secret").
        let (status, _) = json_post(
            &fx.app,
            "/admin/federate",
            &fx.admin_token,
            json!({
                "administratorEmail": "admin@example.org",
                "webOfRegistries": "https://wor.example.org",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Wrong secret is still forbidden.
        let (status, _) = send(
            &fx.app,
            "POST",
            "/updateWebOfRegistries?secret=nope",
            None,
            None,
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // The matching secret enqueues the sync job.
        let (status, body) = send(
            &fx.app,
            "POST",
            "/updateWebOfRegistries?secret=top-secret",
            None,
            None,
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "webhook body: {body}");
        assert_eq!(body, "Registries Updated");
    }

    /// Registry and remote CRUD round-trip through the admin routes.
    #[tokio::test]
    async fn registry_and_remote_crud() {
        let fx = fixture().await;

        // Save a registry entry.
        let (status, _) = json_post(
            &fx.app,
            "/admin/saveRegistry",
            &fx.admin_token,
            json!({ "uri": "https://x.org/", "url": "https://x.org" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Save a valid ICE remote; an unknown type is rejected.
        let (status, _) = json_post(
            &fx.app,
            "/admin/saveRemote",
            &fx.admin_token,
            json!({ "id": "lab-ice", "type": "ice", "url": "https://ice.example.org" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = json_post(
            &fx.app,
            "/admin/saveRemote",
            &fx.admin_token,
            json!({ "id": "bad", "type": "sabre" }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // The remote is listed.
        let (status, body) = send(
            &fx.app,
            "GET",
            "/admin/remotes",
            Some(&fx.admin_token),
            None,
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let remotes: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(remotes["lab-ice"]["type"], "ice");

        // Delete it; a second delete reports it gone.
        let (status, _) = json_post(
            &fx.app,
            "/admin/deleteRemote",
            &fx.admin_token,
            json!({ "id": "lab-ice" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = json_post(
            &fx.app,
            "/admin/deleteRemote",
            &fx.admin_token,
            json!({ "id": "lab-ice" }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
