//! `POST /callPlugin` — proxy a request to a configured plugin.
//!
//! Mirrors classic `lib/plugins/pluginEndpoints.js`: the `message` category
//! echoes, an unknown plugin or endpoint is `404`, and `status` / `evaluate` /
//! `run` proxy to the plugin's matching endpoint. The proxy logic lives in the
//! app-layer [`PluginService`](sbol_db_app::PluginService); this handler parses
//! the request body (JSON or form-encoded, like the other V1 routes) and maps
//! the proxied [`PluginResponse`] and any [`PluginError`] back onto the wire.

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use sbol_db_app::{CallPluginRequest, PluginError, PluginResponse};
use serde::Deserialize;
use serde_json::Value;

use crate::AppState;

/// A JSON `/callPlugin` body: `data` is an arbitrary structured payload.
#[derive(Debug, Default, Deserialize)]
struct CallPluginJson {
    #[serde(default)]
    name: String,
    #[serde(default)]
    endpoint: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    data: Value,
    #[serde(default)]
    prefix: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// A form-encoded `/callPlugin` body: `data` arrives as a string that may itself
/// be JSON, so it is re-parsed when present.
#[derive(Debug, Default, Deserialize)]
struct CallPluginForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    endpoint: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    prefix: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// `POST /callPlugin`.
pub async fn call_plugin(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let req = match parse_request(&headers, &body) {
        Ok(req) => req,
        Err(message) => return text(StatusCode::BAD_REQUEST, message),
    };
    match state.app.plugins().call_plugin(&req).await {
        Ok(response) => into_response(response),
        Err(err) => plugin_error(err),
    }
}

/// Parse the request body as JSON when the `Content-Type` says so, else as a
/// form, matching the rest of the V1 adapter.
fn parse_request(headers: &HeaderMap, body: &[u8]) -> Result<CallPluginRequest, String> {
    let is_json = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/json")
        })
        .unwrap_or(false);
    if is_json {
        let json: CallPluginJson =
            serde_json::from_slice(body).map_err(|e| format!("invalid JSON body: {e}"))?;
        Ok(CallPluginRequest {
            name: json.name,
            endpoint: json.endpoint,
            category: json.category,
            data: json.data,
            prefix: json.prefix,
            message: json.message,
        })
    } else {
        let form: CallPluginForm =
            serde_urlencoded::from_bytes(body).map_err(|e| format!("invalid form body: {e}"))?;
        let data = form
            .data
            .map(|raw| serde_json::from_str::<Value>(&raw).unwrap_or(Value::String(raw)))
            .unwrap_or(Value::Null);
        Ok(CallPluginRequest {
            name: form.name,
            endpoint: form.endpoint,
            category: form.category,
            data,
            prefix: form.prefix,
            message: form.message,
        })
    }
}

/// Turn the proxied [`PluginResponse`] into an axum [`Response`], forwarding the
/// plugin's status plus the `Content-Type` / `Content-Disposition` headers a
/// download plugin sets.
fn into_response(response: PluginResponse) -> Response {
    let status = StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut builder = Response::builder().status(status);
    if let Some(ct) = &response.content_type {
        builder = builder.header(CONTENT_TYPE, ct);
    }
    if let Some(cd) = &response.content_disposition {
        builder = builder.header(CONTENT_DISPOSITION, cd);
    }
    builder
        .body(Body::from(response.body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Map a [`PluginError`] to the classic status code and a `text/plain` body.
fn plugin_error(err: PluginError) -> Response {
    let (status, message) = match err {
        PluginError::NotAuthorized => (StatusCode::FORBIDDEN, err.to_string()),
        PluginError::MissingField(m) => (StatusCode::BAD_REQUEST, m),
        PluginError::NotFound(m) => (StatusCode::NOT_FOUND, m),
        PluginError::Contact(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        PluginError::Domain(d) => (StatusCode::INTERNAL_SERVER_ERROR, d.to_string()),
    };
    text(status, message)
}

fn text(status: StatusCode, body: impl Into<String>) -> Response {
    (status, [(CONTENT_TYPE, "text/plain")], body.into()).into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::{to_bytes, Body};
    use axum::extract::Request;
    use axum::http::header::CONTENT_TYPE;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use axum::Router;
    use sbol_db_app::{AppServices, PluginClient, PluginError, PluginResponse, Registration};
    use sbol_db_backend::Backend;
    use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
    use serde_json::{json, Value};
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::{AppState, Metrics, SchemaCache, ServerConfig};

    /// A stub plugin backed by an in-process axum app: `status` returns a fixed
    /// body and `run`/`evaluate` echo the posted payload back. The
    /// [`PluginClient`] routes every proxied call through it with tower's
    /// `oneshot`, so `/callPlugin` is exercised end to end with no network.
    struct StubPluginClient {
        app: Router,
    }

    impl StubPluginClient {
        fn new() -> Self {
            let app = Router::new()
                .route("/status", get(|| async { "stub-status-ok" }))
                .route(
                    "/run",
                    post(|body: axum::body::Bytes| async move {
                        ([(CONTENT_TYPE, "application/json")], body).into_response()
                    }),
                )
                .route(
                    "/evaluate",
                    post(|body: axum::body::Bytes| async move { body }),
                );
            Self { app }
        }

        async fn oneshot(&self, req: Request<Body>) -> Result<PluginResponse, PluginError> {
            let res = self
                .app
                .clone()
                .oneshot(req)
                .await
                .map_err(|e| PluginError::Contact(e.to_string()))?;
            let status = res.status().as_u16();
            let content_type = res
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let body = to_bytes(res.into_body(), 1024 * 1024)
                .await
                .map_err(|e| PluginError::Contact(e.to_string()))?
                .to_vec();
            Ok(PluginResponse {
                status,
                body,
                content_type,
                content_disposition: None,
            })
        }
    }

    #[async_trait]
    impl PluginClient for StubPluginClient {
        async fn get(&self, url: &str) -> Result<PluginResponse, PluginError> {
            let path = url::Url::parse(url)
                .map_err(|e| PluginError::Contact(e.to_string()))?
                .path()
                .to_owned();
            let req = Request::builder()
                .method("GET")
                .uri(path)
                .body(Body::empty())
                .unwrap();
            self.oneshot(req).await
        }

        async fn post(&self, url: &str, body: &Value) -> Result<PluginResponse, PluginError> {
            let path = url::Url::parse(url)
                .map_err(|e| PluginError::Contact(e.to_string()))?
                .path()
                .to_owned();
            let req = Request::builder()
                .method("POST")
                .uri(path)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap();
            self.oneshot(req).await
        }
    }

    struct Fixture {
        app: Router,
        admin_token: String,
        _dir: TempDir,
    }

    async fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("plugins.db");
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
            AppServices::from_backend(&backend)
                .with_plugin_client(Arc::new(StubPluginClient::new())),
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
        // The full app router, so the jobs API is reachable alongside the
        // SynBioHub V1 surface.
        let app = crate::router(state, ServerConfig::default());
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

    /// Saving a plugin then calling it proxies to the stub and returns the
    /// stub's response, both for `status` (GET) and `run` (POST echo).
    #[tokio::test]
    async fn save_plugin_then_call_plugin_proxies() {
        let fx = fixture().await;

        // Register a rendering plugin pointed at the stub's public HTTPS URL.
        let (status, body) = json_post(
            &fx.app,
            "/admin/savePlugin",
            &fx.admin_token,
            json!({ "category": "rendering", "id": "New", "name": "viz", "url": "https://plugin.example.org" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "savePlugin body: {body}");

        // status proxies a GET to the stub and returns its body.
        let (status, body) = json_post(
            &fx.app,
            "/callPlugin",
            &fx.admin_token,
            json!({ "name": "viz", "endpoint": "status", "category": "rendering" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "callPlugin status body: {body}");
        assert_eq!(body, "stub-status-ok");

        // A curation run echoes the posted payload verbatim (no export-URL
        // augmentation for a non-rendering/download category).
        let (status, _) = json_post(
            &fx.app,
            "/admin/savePlugin",
            &fx.admin_token,
            json!({ "category": "curation", "id": "New", "name": "curate", "url": "https://curate.example.org" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) = json_post(
            &fx.app,
            "/callPlugin",
            &fx.admin_token,
            json!({ "name": "curate", "endpoint": "run", "category": "curation", "data": { "k": "v" } }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "callPlugin run body: {body}");
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap(),
            json!({ "k": "v" })
        );

        // An unknown plugin name is a 404.
        let (status, _) = json_post(
            &fx.app,
            "/callPlugin",
            &fx.admin_token,
            json!({ "name": "nope", "endpoint": "status", "category": "rendering" }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// The SBOLExplorer `updateIndex` shim enqueues a `rebuild_search_index`
    /// job, visible through the jobs API.
    #[tokio::test]
    async fn explorer_update_index_enqueues_rebuild() {
        let fx = fixture().await;

        let (status, body) = json_post(
            &fx.app,
            "/admin/sbolexplorer/updateIndex",
            &fx.admin_token,
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "updateIndex body: {body}");
        let job_id = serde_json::from_str::<Value>(&body).unwrap()["jobId"]
            .as_str()
            .expect("jobId")
            .to_owned();

        // The enqueued job is a rebuild_search_index, per the jobs API.
        let (status, body) = send(
            &fx.app,
            "GET",
            &format!("/jobs/{job_id}"),
            Some(&fx.admin_token),
            None,
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "get job body: {body}");
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap()["kind"],
            "rebuild_search_index"
        );
    }
}
