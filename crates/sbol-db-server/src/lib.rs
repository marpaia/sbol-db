//! HTTP server for sbol-db. Mirrors the CLI surface but over REST.

mod auth;
mod docs;
mod error;
mod explorer;
mod export;
mod instance;
#[cfg(feature = "lab")]
mod lab;
pub mod metrics;
#[cfg(feature = "lab")]
mod portal;
mod routes;
mod serialize;
mod session;
mod synbiohub;
mod v2;

pub use error::ApiError;
pub use export::export_subject_rdf;
#[cfg(feature = "lab")]
pub use lab::SchemaCache;
pub use metrics::Metrics;
#[cfg(feature = "lab")]
pub use sbol_db_storage::{BackendKind, Capabilities, MaintenanceStyle};
pub use serialize::{
    serialize_closure, serialize_gff3, serialize_omex, OmexAttachment, OmexAttachmentSource,
    Serialized,
};

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Request};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use sbol_db_app::AppServices;
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
#[cfg(feature = "lab")]
use sbol_db_storage::{DbStats, LabStore, LsmStats, SqlConsole};
use sbol_db_storage::{JobQueue, SbolStore};
use serde_json::json;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;

#[derive(Clone)]
pub struct AppState {
    pub service: Arc<dyn SbolStore>,
    pub sparql: Arc<SparqlEngine>,
    pub sparql_update: Arc<SparqlUpdateEngine>,
    /// The backend-neutral application facade the identity-aware adapters share
    /// (ACL scoping, and the net-new subsystems added in later phases). Wired
    /// in alongside the existing raw handles; the current routes are unchanged.
    pub app: Arc<AppServices>,
    pub metrics: Arc<Metrics>,
    pub jobs: Arc<dyn JobQueue>,
    /// Backend-neutral dashboard / graph-browser reads for the lab UI.
    #[cfg(feature = "lab")]
    pub lab: Arc<dyn LabStore>,
    /// Runtime configuration visible to handlers (lab SQL limits, etc).
    /// Cloned in once at server startup; never mutated.
    pub config: ServerConfig,
    /// Which engine is running, so the lab can advertise its capabilities.
    #[cfg(feature = "lab")]
    pub backend_kind: BackendKind,
    /// The SQL console, when the engine is a SQL database (Postgres, SQLite).
    /// `None` on a key-value backend; the SQL pages degrade with a clear
    /// "backend unsupported" error (see [`AppState::require_sql_console`]).
    #[cfg(feature = "lab")]
    pub sql_console: Option<Arc<dyn SqlConsole>>,
    /// Relational-engine introspection (tables, indexes, schema, sessions),
    /// when the backend has one (Postgres, SQLite).
    #[cfg(feature = "lab")]
    pub db_stats: Option<Arc<dyn DbStats>>,
    /// LSM-engine introspection (column families, levels, compaction), when
    /// the backend is a key-value store (RocksDB).
    #[cfg(feature = "lab")]
    pub lsm_stats: Option<Arc<dyn LsmStats>>,
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
    /// When true (and the `lab` cargo feature is enabled), embedded application
    /// assets, the transitional `/lab` entry, and `/lab/api` are available.
    /// The root application additionally requires `portal_enabled`. This toggle is
    /// runtime-only — to strip the UI from the binary entirely, build with
    /// `--no-default-features` on `sbol-db-server`.
    pub lab_enabled: bool,
    /// When true (and the UI is enabled), wrap the completed HTTP router with
    /// compatibility-aware browser dispatch for the root-mounted SBOL DB
    /// application. Set false to disable browser pages while retaining every API
    /// route and the transitional `/lab` mount.
    pub portal_enabled: bool,
    /// Add the `Secure` attribute to the shared browser-session cookie. Keep
    /// this false for plain-HTTP local development and enable it for every
    /// deployment whose public origin is HTTPS.
    pub session_cookie_secure: bool,
    /// Require an authenticated administrator for `/lab/api/*`. This defaults
    /// on. The false value exists for isolated handler fixtures and deliberate
    /// compatibility deployments; it should not be used on a public server.
    pub admin_api_auth_required: bool,
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
    /// Salt the legacy SynBioHub password digest `sha1(salt + sha1(pw))` was
    /// computed with, used to verify migrated credentials on the V1 auth
    /// routes. Defaults to classic's `synbiohub_change_me`; a migrated instance
    /// sets `SBOL_DB_PASSWORD_SALT` to its original `passwordSalt`.
    pub password_salt: String,
    /// Whether `POST /register` accepts self-service account creation. When
    /// false the route returns `403`, matching classic's `allowPublicSignup`.
    pub allow_public_signup: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            // Slightly higher than the SPARQL default (30s) so SPARQL
            // returns its 504-equivalent before the outer timer fires.
            request_timeout: Duration::from_secs(60),
            max_body_bytes: 32 * 1024 * 1024,
            lab_enabled: true,
            portal_enabled: true,
            session_cookie_secure: false,
            admin_api_auth_required: true,
            lab_sql_timeout_ms_max: 60_000,
            lab_sql_row_cap_max: 50_000,
            sparql_auth_user: "dba".to_owned(),
            sparql_auth_password: "dba".to_owned(),
            sparql_auth_disabled: false,
            password_salt: "synbiohub_change_me".to_owned(),
            allow_public_signup: true,
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
            portal_enabled: std::env::var("SBOL_DB_PORTAL_ENABLED")
                .ok()
                .map(|v| parse_bool(&v))
                .unwrap_or(defaults.portal_enabled),
            session_cookie_secure: std::env::var("SBOL_DB_SESSION_COOKIE_SECURE")
                .ok()
                .map(|v| parse_bool(&v))
                .unwrap_or(defaults.session_cookie_secure),
            admin_api_auth_required: std::env::var("SBOL_DB_ADMIN_API_AUTH_REQUIRED")
                .ok()
                .map(|v| parse_bool(&v))
                .unwrap_or(defaults.admin_api_auth_required),
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
            password_salt: std::env::var("SBOL_DB_PASSWORD_SALT").unwrap_or(defaults.password_salt),
            allow_public_signup: std::env::var("SBOL_DB_ALLOW_PUBLIC_SIGNUP")
                .ok()
                .map(|v| parse_bool(&v))
                .unwrap_or(defaults.allow_public_signup),
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

    /// What the running backend can do, for the lab `/info` endpoint and the
    /// UI's feature gating. Derived from which capability handles the backend
    /// populated, plus the engine-specific refinements (slow-query stats and
    /// live sessions/locks are Postgres-only).
    #[cfg(feature = "lab")]
    pub fn capabilities(&self) -> Capabilities {
        let maintenance = if self.db_stats.is_some() {
            Some(MaintenanceStyle::Relational)
        } else if self.lsm_stats.is_some() {
            Some(MaintenanceStyle::Lsm)
        } else {
            None
        };
        let is_postgres = self.backend_kind == BackendKind::Postgres;
        Capabilities {
            sql_console: self.sql_console.is_some(),
            relational_schema: self.db_stats.is_some(),
            maintenance,
            slow_query_stats: is_postgres,
            activity_and_locks: is_postgres,
        }
    }

    /// The SQL console, or a "backend unsupported" error. The SQL execute /
    /// validate pages call this so they degrade cleanly on a key-value
    /// backend.
    #[cfg(feature = "lab")]
    pub fn require_sql_console(&self) -> Result<&Arc<dyn SqlConsole>, ApiError> {
        self.sql_console
            .as_ref()
            .ok_or_else(|| ApiError::Unavailable(SQL_UNSUPPORTED.to_owned()))
    }

    /// Relational-engine introspection, or a "backend unsupported" error.
    #[cfg(feature = "lab")]
    pub fn require_db_stats(&self) -> Result<&Arc<dyn DbStats>, ApiError> {
        self.db_stats.as_ref().ok_or_else(|| {
            ApiError::Unavailable(
                "this lab page requires a relational engine (Postgres or SQLite); the server is \
                 running on a different backend"
                    .to_owned(),
            )
        })
    }

    /// LSM-engine introspection, or a "backend unsupported" error.
    #[cfg(feature = "lab")]
    pub fn require_lsm_stats(&self) -> Result<&Arc<dyn LsmStats>, ApiError> {
        self.lsm_stats.as_ref().ok_or_else(|| {
            ApiError::Unavailable(
                "this lab page requires the RocksDB backend; the server is running on a \
                 different backend"
                    .to_owned(),
            )
        })
    }
}

#[cfg(feature = "lab")]
const SQL_UNSUPPORTED: &str =
    "the SQL console requires a SQL engine (Postgres or SQLite); the server is running on a \
     key-value backend";

pub fn router(state: AppState, config: ServerConfig) -> Router {
    let api = Router::new()
        .route("/healthz", get(routes::healthz))
        .route("/readyz", get(routes::readyz))
        .route("/metrics", get(metrics::metrics_handler))
        .route("/docs", get(docs::docs_html))
        .route("/openapi.json", get(docs::openapi_json))
        .route("/synbiohub/openapi.json", get(docs::synbiohub_openapi_json))
        .route(
            "/graphs",
            post(routes::create_graph).delete(routes::delete_graph_by_document_iri),
        )
        .route("/graphs/bulk", post(routes::create_graphs_bulk))
        .route(
            "/graphs/:id",
            get(routes::get_graph).delete(routes::delete_graph),
        )
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
        // The idiomatic V2 REST surface, a second presentation of the same
        // facade under a versioned prefix. It carries its own bearer-token
        // identity layer and inherits the metrics/body-limit/timeout layers.
        .nest("/api/v2", v2::router(state.clone()))
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

    // The SynBioHub V1 auth surface (`/login`, `/register`, `/profile`, …),
    // behind the `X-authorization` middleware. It is independent of the
    // Basic-auth `/sparql-auth*` write path above.
    let synbiohub_routes = synbiohub::router(state.clone());

    let app = mount_portal(
        mount_lab(api.merge(authed).merge(synbiohub_routes), &config, &state)
            .fallback(not_found_handler)
            .with_state(state),
        &config,
    );

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
        // Permissive CORS so a browser SPA (e.g. the SynBioHub frontend, which
        // calls this V1 API cross-origin) can drive the API, matching classic
        // SynBioHub's `app.use(cors())`.
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
}

/// The network-internal SBOLExplorer wire-compatibility surface. It shares the
/// exact [`AppState`] used by [`router`], including the process-local ranked
/// index, cluster data, and embedded worker queue. Deployments normally bind
/// this router to port 13162 and alias the service as `explorer`.
pub fn explorer_router(state: AppState, config: ServerConfig) -> Router {
    Router::new()
        .route("/", get(explorer::endpoint))
        .route(
            "/config",
            get(explorer::get_config).post(explorer::set_config),
        )
        .route("/update", get(explorer::update))
        .route("/incrementalupdate", post(explorer::incremental_update))
        .route("/incrementalremove", get(explorer::incremental_remove))
        .route(
            "/incrementalremovecollection",
            get(explorer::incremental_remove_collection),
        )
        .route("/info", get(explorer::info))
        .route("/indexinginfo", get(explorer::indexing_info))
        .route("/healthz", get(routes::healthz))
        .route("/readyz", get(routes::readyz))
        .route("/metrics", get(metrics::metrics_handler))
        .route_layer(axum::middleware::from_fn(metrics::track_explorer_metrics))
        .fallback(not_found_handler)
        .with_state(state)
        .layer(DefaultBodyLimit::max(config.max_body_bytes))
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
fn mount_lab(
    router: Router<AppState>,
    config: &ServerConfig,
    state: &AppState,
) -> Router<AppState> {
    if !config.lab_enabled {
        tracing::info!("lab disabled via SBOL_DB_LAB_ENABLED");
        return router;
    }
    tracing::info!(
        ui_built = sbol_db_ui::is_built(),
        "UI enabled; mounting admin JSON API at /lab/api and transitional SPA entry at /lab"
    );
    // Nest /lab/api first: axum matches more specific prefixes ahead
    // of shorter ones, but registering in order keeps the intent
    // legible and avoids surprises if axum's matcher ever changes.
    let lab_api = if config.admin_api_auth_required {
        lab::router()
            .route_layer(axum::middleware::from_fn(lab::require_admin))
            .route_layer(axum::middleware::from_fn_with_state(
                state.clone(),
                v2::auth::attach_identity,
            ))
    } else {
        tracing::warn!("admin API authentication disabled");
        lab::router()
    };
    router
        .nest("/lab/api", lab_api)
        .nest_service("/lab", sbol_db_ui::router())
}

#[cfg(not(feature = "lab"))]
fn mount_lab(
    router: Router<AppState>,
    _config: &ServerConfig,
    _state: &AppState,
) -> Router<AppState> {
    router
}

#[cfg(feature = "lab")]
fn mount_portal(router: Router, config: &ServerConfig) -> Router {
    if !config.lab_enabled {
        return router;
    }
    if !config.portal_enabled {
        tracing::info!(
            "public portal pages disabled via SBOL_DB_PORTAL_ENABLED; retaining admin UI"
        );
        return router.layer(axum::middleware::from_fn(portal::dispatch_admin));
    }
    tracing::info!(
        ui_built = sbol_db_ui::is_built(),
        "portal enabled; browser navigation is served from the root origin"
    );
    // Apply this after routes, fallback, and state are complete. The layer must
    // see a request before Axum selects a V1 handler or emits a method-specific
    // 405; paths such as GET /profile and GET /login exercise both cases.
    router.layer(axum::middleware::from_fn(portal::dispatch))
}

#[cfg(not(feature = "lab"))]
fn mount_portal(router: Router, _config: &ServerConfig) -> Router {
    router
}
