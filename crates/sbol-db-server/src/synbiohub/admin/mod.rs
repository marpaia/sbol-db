//! Admin surface for the SynBioHub v1 adapter.
//!
//! Every route in this module is admin-gated: [`require_admin`] rejects an
//! anonymous caller with `401` and a signed-in non-administrator with `403`
//! before the handler runs, matching classic SynBioHub's `requireAdmin` gate on
//! the `/admin/*` actions.
//! The gate is security-critical, so it is applied as a route layer to the
//! whole admin router rather than repeated per handler.
//!
//! The routes cover the instance dashboard, the durable configuration sections
//! (mail, theme, user-signup policy), admin-scope data reads (every graph, an
//! all-graph SPARQL endpoint), the log tail, the search-index rebuild, and user
//! CRUD over the identity [`UserStore`](sbol_db_storage::UserStore).

mod config_routes;
mod data_routes;
mod explorer_routes;
mod federation_routes;
mod plugins_routes;
mod users_routes;

pub use federation_routes::update_web_of_registries;
// Handlers classic serves publicly (theme/plugins/registries GET) or that
// self-authorize (theme POST), mounted on the public router by [`super::router`]
// so an anonymous browser can read them, matching classic's gates.
pub(super) use config_routes::{get_theme, set_theme};
pub(super) use federation_routes::registries;
pub(super) use plugins_routes::plugins;

use axum::extract::{Extension, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use sbol_db_app::ConfigError;
use sbol_db_storage::{EnqueueOutcome, NewJob};
use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};

use super::{attach_current_user, CurrentUser};
use crate::error::ApiError;
use crate::AppState;

/// The job kind the search-index rebuild handler is registered under.
const REINDEX_KIND: &str = "rebuild_search_index";

/// The admin router: every route runs behind [`require_admin`] (which itself
/// runs behind [`attach_current_user`], so the caller identity is resolved
/// before the gate reads it). Merged into the V1 adapter router by
/// [`super::router`].
pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/admin", get(dashboard))
        .route("/admin/graphs", get(data_routes::graphs))
        .route("/admin/sparql", get(data_routes::sparql))
        .route("/admin/log", get(config_routes::log))
        .route("/admin/jobs", get(super::jobs::admin_jobs))
        .route("/admin/virtuoso", get(super::jobs::virtuoso))
        .route("/admin/listLogs", get(super::jobs::list_logs))
        .route(
            "/admin/mail",
            get(config_routes::get_mail).post(config_routes::set_mail),
        )
        .route(
            "/admin/users",
            get(config_routes::get_users_config).post(config_routes::set_users_config),
        )
        .route("/admin/reindex", post(reindex))
        .route("/admin/newUser", post(users_routes::create_user))
        .route("/admin/updateUser", post(users_routes::update_user))
        .route("/admin/deleteUser", post(users_routes::delete_user))
        // Web of Registries federation and ICE/Benchling remotes.
        .route("/admin/federate", post(federation_routes::federate))
        .route(
            "/admin/retrieveFromWebOfRegistries",
            post(federation_routes::retrieve),
        )
        .route(
            "/admin/saveRegistry",
            post(federation_routes::save_registry),
        )
        .route(
            "/admin/deleteRegistry",
            post(federation_routes::delete_registry),
        )
        .route("/admin/remotes", get(federation_routes::remotes))
        .route("/admin/saveRemote", post(federation_routes::save_remote))
        .route(
            "/admin/deleteRemote",
            post(federation_routes::delete_remote),
        )
        // External plugins (rendering/download/submit/curation/authorization).
        .route("/admin/savePlugin", post(plugins_routes::save_plugin))
        .route("/admin/deletePlugin", post(plugins_routes::delete_plugin))
        // SBOLExplorer shims over the native search engine, at classic's paths.
        .route(
            "/admin/explorerUpdateIndex",
            post(explorer_routes::update_index),
        )
        .route(
            "/admin/explorer",
            get(explorer_routes::get_config).post(explorer_routes::set_config),
        )
        .route("/admin/explorerLog", get(explorer_routes::log))
        .route("/admin/explorerIndexingLog", get(explorer_routes::log))
        // The gate runs after identity resolution: `route_layer` wraps
        // outermost-last, so `attach_current_user` populates `CurrentUser`
        // first and `require_admin` reads it.
        .route_layer(axum::middleware::from_fn(require_admin))
        .route_layer(axum::middleware::from_fn_with_state(
            state,
            attach_current_user,
        ))
}

/// Gate an admin route: an authenticated administrator passes; a signed-in
/// non-admin is `403 Forbidden`; an anonymous caller is `401 Unauthorized`.
/// Distinguishing the two matches classic and standard HTTP semantics, so a UI
/// that gets `401` knows to prompt for login rather than show "forbidden".
async fn require_admin(
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    match user {
        Some(user) if user.is_admin => next.run(req).await,
        Some(_) => forbidden("administrator privileges are required"),
        None => unauthorized("authentication is required"),
    }
}

/// `GET /admin` — the instance status summary: every persisted configuration
/// section keyed by name. Classic renders this as the admin dashboard; the
/// adapter returns it as JSON.
async fn dashboard(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let config: Map<String, Value> = state
        .app
        .config_service()
        .get_all()
        .await?
        .into_iter()
        .map(|entry| (entry.key, entry.value))
        .collect();
    Ok(Json(json!({ "status": "ok", "config": config })))
}

/// `POST /admin/reindex` — the SynBioHub `updateIndex` analogue: enqueue a
/// full ranked-search-index rebuild and return the job id for polling. The
/// rebuild runs on the worker, recomputing PageRank and rebuilding the ranked
/// text index. SBOLExplorer is internal now, so this native shim replaces its
/// external index-update call.
pub async fn reindex(State(state): State<AppState>) -> Response {
    let job = NewJob {
        kind: REINDEX_KIND.to_owned(),
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
        Ok(EnqueueOutcome::Inserted(job)) | Ok(EnqueueOutcome::AlreadyExists(job)) => {
            (StatusCode::ACCEPTED, Json(json!({ "jobId": job.id }))).into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("enqueue reindex job: {err}"),
        )
            .into_response(),
    }
}

/// A `403` with a plain-text body, the shape the admin gate and a rejected
/// configuration write share.
pub(super) fn forbidden(message: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        [(CONTENT_TYPE, "text/plain")],
        message.to_owned(),
    )
        .into_response()
}

/// A `401` with a plain-text body, the admin gate's response to an anonymous
/// caller (classic distinguishes this from a signed-in non-admin's `403`).
fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(CONTENT_TYPE, "text/plain")],
        message.to_owned(),
    )
        .into_response()
}

/// Map a [`ConfigError`] to the adapter error: an authorization failure is a
/// `403`, anything else surfaces as its underlying domain error.
pub(super) fn config_err(err: ConfigError) -> ApiError {
    match err {
        ConfigError::NotAuthorized => ApiError::Forbidden(err.to_string()),
        ConfigError::Domain(domain) => ApiError::Domain(domain),
    }
}

/// The lowercased base media type of a request body.
fn content_type(headers: &HeaderMap) -> String {
    headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default()
}

/// Deserialize a request body as JSON when the `Content-Type` says so, else as
/// form-encoded, mirroring the V1 auth routes so classic form posts and JSON
/// API clients both work.
pub(super) fn parse_body<T: DeserializeOwned>(
    headers: &HeaderMap,
    body: &[u8],
) -> Result<T, ApiError> {
    if content_type(headers) == "application/json" {
        serde_json::from_slice(body)
            .map_err(|e| ApiError::BadRequest(format!("invalid JSON body: {e}")))
    } else {
        serde_urlencoded::from_bytes(body)
            .map_err(|e| ApiError::BadRequest(format!("invalid form body: {e}")))
    }
}

/// Parse a configuration-section body into a JSON object: a JSON body is taken
/// verbatim, a form body becomes an object of string-valued fields. Classic
/// posts the config sections as `application/x-www-form-urlencoded`.
pub(super) fn parse_config_value(headers: &HeaderMap, body: &[u8]) -> Result<Value, ApiError> {
    if content_type(headers) == "application/json" {
        serde_json::from_slice(body)
            .map_err(|e| ApiError::BadRequest(format!("invalid JSON body: {e}")))
    } else {
        let fields: std::collections::BTreeMap<String, String> = serde_urlencoded::from_bytes(body)
            .map_err(|e| ApiError::BadRequest(format!("invalid form body: {e}")))?;
        Ok(Value::Object(
            fields
                .into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use sbol_db_app::{AppServices, Registration};
    use sbol_db_backend::Backend;
    use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
    use serde_json::{json, Value};
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::{synbiohub, AppState, Metrics, SchemaCache, ServerConfig};

    /// A live server router over a fresh SQLite backend plus the admin and
    /// member tokens minted against it.
    struct Fixture {
        app: Router,
        admin_token: String,
        member_token: String,
        _dir: TempDir,
    }

    async fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("admin.db");
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
        let app_services = Arc::new(AppServices::from_backend(&backend));

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

        let member = app_services
            .auth
            .register(Registration {
                username: "member".to_owned(),
                name: "Member".to_owned(),
                email: "member@example.org".to_owned(),
                affiliation: None,
                password: "s3cret".to_owned(),
                is_admin: false,
                is_curator: false,
                is_member: true,
            })
            .await
            .expect("register member");
        let member_token = app_services
            .auth
            .issue_token(member.id)
            .await
            .expect("issue member token");

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
            member_token,
            _dir: dir,
        }
    }

    /// Every admin-gated route, as `(method, path)`. The gate runs before body
    /// parsing, so an empty body is fine for the rejection checks. The
    /// `theme`/`plugins`/`registries` GETs are deliberately absent: classic
    /// serves them to anonymous callers so the UI can render, so they live in
    /// the public router and are covered by [`public_config_gets_are_anonymous`].
    fn admin_routes() -> Vec<(&'static str, &'static str)> {
        vec![
            ("GET", "/admin"),
            ("GET", "/admin/graphs"),
            ("GET", "/admin/sparql?query=ASK%20%7B%7D"),
            ("GET", "/admin/log"),
            ("GET", "/admin/mail"),
            ("POST", "/admin/mail"),
            ("POST", "/admin/theme"),
            ("GET", "/admin/users"),
            ("POST", "/admin/users"),
            ("POST", "/admin/reindex"),
            ("POST", "/admin/newUser"),
            ("POST", "/admin/updateUser"),
            ("POST", "/admin/deleteUser"),
            ("POST", "/admin/federate"),
            ("POST", "/admin/retrieveFromWebOfRegistries"),
            ("POST", "/admin/saveRegistry"),
            ("POST", "/admin/deleteRegistry"),
            ("GET", "/admin/remotes"),
            ("POST", "/admin/saveRemote"),
            ("POST", "/admin/deleteRemote"),
            ("POST", "/admin/savePlugin"),
            ("POST", "/admin/deletePlugin"),
            ("POST", "/admin/explorerUpdateIndex"),
            ("GET", "/admin/explorer"),
            ("POST", "/admin/explorer"),
            ("GET", "/admin/explorerLog"),
        ]
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

    #[tokio::test]
    async fn admin_routes_gate_anonymous_401_and_non_admin_403() {
        let fx = fixture().await;
        for (method, path) in admin_routes() {
            // Anonymous: 401 Unauthorized (no credentials).
            let (status, _) = send(&fx.app, method, path, None, None, Body::empty()).await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "anonymous {method} {path} must be 401"
            );
            // Authenticated non-admin: 403 Forbidden (credentials, no rights).
            let (status, _) = send(
                &fx.app,
                method,
                path,
                Some(&fx.member_token),
                None,
                Body::empty(),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "non-admin {method} {path} must be 403"
            );
        }
    }

    #[tokio::test]
    async fn public_config_gets_are_anonymous() {
        let fx = fixture().await;
        // Classic serves these branding/registry reads to anonymous callers so
        // the UI can render before login; sbol-db mounts them publicly too.
        for path in ["/admin/theme", "/admin/plugins", "/admin/registries"] {
            let (status, _) = send(&fx.app, "GET", path, None, None, Body::empty()).await;
            assert_eq!(status, StatusCode::OK, "anonymous GET {path} must be 200");
        }
    }

    #[tokio::test]
    async fn admin_reads_status_and_sets_config() {
        let fx = fixture().await;

        // The dashboard is reachable for an admin.
        let (status, _) = send(
            &fx.app,
            "GET",
            "/admin",
            Some(&fx.admin_token),
            None,
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // A config section is absent until written.
        let (status, body) = send(
            &fx.app,
            "GET",
            "/admin/mail",
            Some(&fx.admin_token),
            None,
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap(),
            Value::Null,
            "unset mail config reads back as null"
        );

        // An admin write persists and reads back verbatim.
        let (status, _) = json_post(
            &fx.app,
            "/admin/mail",
            &fx.admin_token,
            json!({ "fromAddress": "ops@example.org", "sendgridApiKey": "sg-1" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, body) = send(
            &fx.app,
            "GET",
            "/admin/mail",
            Some(&fx.admin_token),
            None,
            Body::empty(),
        )
        .await;
        let stored: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(stored["fromAddress"], "ops@example.org");
    }

    #[tokio::test]
    async fn admin_creates_updates_and_deletes_a_user() {
        let fx = fixture().await;

        // Create.
        let (status, body) = json_post(
            &fx.app,
            "/admin/newUser",
            &fx.admin_token,
            json!({
                "username": "dana",
                "name": "Dana",
                "email": "dana@example.org",
                "password": "s3cret",
                "isCurator": true,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create body: {body}");
        let created: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(created["username"], "dana");
        assert_eq!(created["isCurator"], true);

        // Update the affiliation.
        let (status, body) = json_post(
            &fx.app,
            "/admin/updateUser",
            &fx.admin_token,
            json!({ "username": "dana", "affiliation": "Acme Bio" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "update body: {body}");
        let updated: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(updated["affiliation"], "Acme Bio");

        // Delete, then confirm a second delete reports the account gone.
        let (status, _) = json_post(
            &fx.app,
            "/admin/deleteUser",
            &fx.admin_token,
            json!({ "username": "dana" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = json_post(
            &fx.app,
            "/admin/deleteUser",
            &fx.admin_token,
            json!({ "username": "dana" }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
