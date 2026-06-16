//! HTTP server for sbol-db. Mirrors the CLI surface but over REST.

mod auth;
mod docs;
mod error;
mod export;
#[cfg(feature = "lab")]
mod lab;
pub mod metrics;
mod routes;

pub use error::ApiError;
pub use export::export_subject_rdf;
#[cfg(feature = "lab")]
pub use lab::SchemaCache;
pub use metrics::Metrics;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Request};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use sbol_db_storage::{JobQueue, SbolStore};
use serde_json::json;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;

#[derive(Clone)]
pub struct AppState {
    pub service: Arc<dyn SbolStore>,
    pub sparql: Arc<SparqlEngine>,
    pub sparql_update: Arc<SparqlUpdateEngine>,
    pub metrics: Arc<Metrics>,
    pub jobs: Arc<dyn JobQueue>,
    /// Runtime configuration visible to handlers (lab SQL limits, etc).
    /// Cloned in once at server startup; never mutated.
    pub config: ServerConfig,
    /// Postgres connection handle for the lab's SQL and introspection
    /// endpoints, which are irreducibly Postgres-specific. Present whenever
    /// the `lab` feature and a Postgres backend are active.
    #[cfg(feature = "lab")]
    pub pg_pool: sbol_db_postgres::PgPool,
    /// Per-process TTL cache for the `/lab/api/schema/*` endpoints.
    #[cfg(feature = "lab")]
    pub schema_cache: Arc<lab::SchemaCache>,
}

/// Operational limits applied to every route. The outer
/// `request_timeout` is a wall-clock bound on the whole request; SPARQL
/// has its own (shorter) cooperative timeout inside `SparqlOptions`.
/// `max_body_bytes` rejects oversize POST bodies before they're read
/// into memory.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub request_timeout: Duration,
    pub max_body_bytes: usize,
    /// When true (and the `lab` cargo feature is enabled), the data lab
    /// bench SPA is mounted at `/lab` and its JSON API at `/lab/api`.
    /// The toggle is runtime-only — to strip the embedded UI from the
    /// binary entirely, build with `--no-default-features` on
    /// `sbol-db-server`.
    pub lab_enabled: bool,
    /// Upper bound (ms) the lab SQL endpoint applies via
    /// `SET LOCAL statement_timeout`. Clients can ask for less, never
    /// more.
    pub lab_sql_timeout_ms_max: u64,
    /// Upper bound on the row count the lab SQL endpoint will return
    /// in one response payload. Rows beyond this are dropped with
    /// `truncated = true`.
    pub lab_sql_row_cap_max: u32,
    /// Credentials the authenticated SPARQL endpoints (`/sparql-auth`,
    /// `/sparql-graph-crud-auth/`) require via HTTP Basic. Default `dba`/`dba`
    /// matches Virtuoso, so SynBioHub needs no config change. When
    /// `sparql_auth_disabled` is true the endpoints skip auth entirely (for
    /// trusted-network deployments behind a proxy).
    pub sparql_auth_user: String,
    pub sparql_auth_password: String,
    pub sparql_auth_disabled: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            // Slightly higher than the SPARQL default (30s) so SPARQL
            // returns its 504-equivalent before the outer timer fires.
            request_timeout: Duration::from_secs(60),
            max_body_bytes: 32 * 1024 * 1024,
            lab_enabled: true,
            lab_sql_timeout_ms_max: 60_000,
            lab_sql_row_cap_max: 50_000,
            sparql_auth_user: "dba".to_owned(),
            sparql_auth_password: "dba".to_owned(),
            sparql_auth_disabled: false,
        }
    }
}

impl ServerConfig {
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            request_timeout: std::env::var("SBOL_DB_REQUEST_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .map(Duration::from_secs)
                .unwrap_or(defaults.request_timeout),
            max_body_bytes: std::env::var("SBOL_DB_MAX_BODY_BYTES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(defaults.max_body_bytes),
            lab_enabled: std::env::var("SBOL_DB_LAB_ENABLED")
                .ok()
                .map(|v| parse_bool(&v))
                .unwrap_or(defaults.lab_enabled),
            lab_sql_timeout_ms_max: std::env::var("SBOL_DB_LAB_SQL_TIMEOUT_MS_MAX")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(defaults.lab_sql_timeout_ms_max),
            lab_sql_row_cap_max: std::env::var("SBOL_DB_LAB_SQL_ROW_CAP_MAX")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(defaults.lab_sql_row_cap_max),
            sparql_auth_user: std::env::var("SBOL_DB_SPARQL_AUTH_USER")
                .unwrap_or(defaults.sparql_auth_user),
            sparql_auth_password: std::env::var("SBOL_DB_SPARQL_AUTH_PASSWORD")
                .unwrap_or(defaults.sparql_auth_password),
            sparql_auth_disabled: std::env::var("SBOL_DB_SPARQL_AUTH_DISABLED")
                .ok()
                .map(|v| parse_bool(&v))
                .unwrap_or(defaults.sparql_auth_disabled),
        }
    }
}

fn parse_bool(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

impl AppState {
    /// Drop any cached lab payloads. Called from handlers that mutate
    /// state visible through the lab API (e.g. ontology loads change
    /// the SPARQL schema and overview ontology count). No-op when the
    /// `lab` feature is off.
    pub fn invalidate_lab_caches(&self) {
        #[cfg(feature = "lab")]
        {
            let cache = self.schema_cache.clone();
            tokio::spawn(async move { cache.invalidate_all().await });
        }
    }
}

pub fn router(state: AppState, config: ServerConfig) -> Router {
    let api = Router::new()
        .route("/healthz", get(routes::healthz))
        .route("/readyz", get(routes::readyz))
        .route("/metrics", get(metrics::metrics_handler))
        .route("/docs", get(docs::docs_html))
        .route("/openapi.json", get(docs::openapi_json))
        .route("/graphs", post(routes::create_graph))
        .route("/graphs/bulk", post(routes::create_graphs_bulk))
        .route("/graphs/:id", get(routes::get_graph))
        .route("/objects", get(routes::get_object_by_iri))
        .route("/objects/list", get(routes::list_objects))
        .route("/objects/lookup", post(routes::lookup_objects))
        .route("/objects/:id/rdf", get(routes::export_object))
        .route("/objects/neighborhood", get(routes::neighborhood))
        .route("/objects/neighborhood.rdf", get(routes::neighborhood_rdf))
        .route("/sparql", get(routes::sparql_get).post(routes::sparql_post))
        .route(
            "/sequences/search",
            get(routes::sequence_search).post(routes::sequence_search_batch),
        )
        .route(
            "/ontology",
            get(routes::ontology_list).post(routes::ontology_load),
        )
        .route("/ontology/term", get(routes::ontology_term))
        .route("/ontology/terms", get(routes::ontology_terms))
        .route("/ontology/descendants", get(routes::ontology_descendants))
        .route("/jobs", get(routes::list_jobs).post(routes::enqueue_job))
        .route("/jobs/:id", get(routes::get_job))
        .route("/jobs/:id/attempts", get(routes::list_job_attempts))
        .route("/jobs/:id/logs", get(routes::list_job_logs))
        .route("/jobs/:id/cancel", post(routes::cancel_job))
        .route_layer(axum::middleware::from_fn(metrics::track_metrics));

    // SynBioHub/Virtuoso-compatible write surface, behind HTTP Basic auth.
    // Registered on both the bare and trailing-slash paths because SynBioHub
    // configures `…/sparql-graph-crud-auth/` with the slash.
    let graph_crud = get(routes::graph_store_get)
        .post(routes::graph_store_post)
        .put(routes::graph_store_put)
        .delete(routes::graph_store_delete);
    let authed = Router::new()
        .route(
            "/sparql-auth",
            get(routes::sparql_auth).post(routes::sparql_auth),
        )
        .route("/sparql-graph-crud-auth", graph_crud.clone())
        .route("/sparql-graph-crud-auth/", graph_crud)
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));

    let app = mount_lab(api.merge(authed), &config)
        .fallback(not_found_handler)
        .with_state(state);

    // Body limit and timeout apply to every route, including the
    // operational endpoints. `DefaultBodyLimit::max` overrides axum's
    // built-in 2 MiB default; `RequestBodyLimitLayer` is the hard
    // cap that rejects oversize bodies with 413 before they're
    // streamed into memory.
    app.layer(DefaultBodyLimit::max(config.max_body_bytes))
        .layer(RequestBodyLimitLayer::new(config.max_body_bytes))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            config.request_timeout,
        ))
}

/// Catch-all that logs unmatched requests and returns a JSON-shaped
/// 404. Axum's default 404 is silent and bodyless, which makes "why is
/// the UI getting Not Found?" hard to debug. The log line lands at WARN
/// so it shows up in normal `cargo run` output.
async fn not_found_handler(req: Request) -> impl IntoResponse {
    let method = req.method().clone();
    let uri = req.uri().clone();
    tracing::warn!(%method, path = %uri.path(), "404: no route matched");
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "type": "not_found",
            "title": "not_found",
            "status": 404,
            "detail": format!("no route registered for {method} {}", uri.path()),
        })),
    )
}

#[cfg(feature = "lab")]
fn mount_lab(router: Router<AppState>, config: &ServerConfig) -> Router<AppState> {
    if !config.lab_enabled {
        tracing::info!("lab disabled via SBOL_DB_LAB_ENABLED");
        return router;
    }
    tracing::info!(
        ui_built = sbol_db_ui::is_built(),
        "lab enabled; mounting JSON API at /lab/api and SPA at /lab"
    );
    // Nest /lab/api first: axum matches more specific prefixes ahead
    // of shorter ones, but registering in order keeps the intent
    // legible and avoids surprises if axum's matcher ever changes.
    router
        .nest("/lab/api", lab::router())
        .nest_service("/lab", sbol_db_ui::router())
}

#[cfg(not(feature = "lab"))]
fn mount_lab(router: Router<AppState>, _config: &ServerConfig) -> Router<AppState> {
    router
}
