//! Prometheus metrics surface for the HTTP server.
//!
//! A single process-global recorder is installed on first call to
//! [`Metrics::install`]; subsequent calls reuse the same handle so unit
//! tests that build multiple routers don't panic on the second install.
//! Cardinality is bounded: HTTP labels use `axum::extract::MatchedPath`
//! to template the route (e.g. `/objects/:id`, not the raw IRI), and
//! requests that didn't match a route are bucketed as `unmatched`.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use axum::extract::{MatchedPath, Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use sbol_db_postgres::PgPool;
use sbol_db_storage::{JobQueue, JobStatus};
use serde::Serialize;

use crate::AppState;

const HTTP_DURATION_BUCKETS_SECONDS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

const JOB_DURATION_BUCKETS_SECONDS: &[f64] =
    &[0.01, 0.05, 0.25, 1.0, 5.0, 15.0, 60.0, 300.0, 900.0, 3600.0];

const JOB_WAIT_BUCKETS_SECONDS: &[f64] =
    &[0.01, 0.1, 0.5, 1.0, 5.0, 15.0, 60.0, 300.0, 900.0, 3600.0];

pub struct Metrics {
    handle: PrometheusHandle,
    /// The API connection pool, when the backend exposes one (Postgres).
    /// `None` for poolless backends; pool gauges are then simply not emitted.
    api_pool: Option<PgPool>,
    worker_pool: std::sync::Mutex<Option<PgPool>>,
    jobs: std::sync::Mutex<Option<Arc<dyn JobQueue>>>,
    data_disk: std::sync::Mutex<Option<DataDiskProbe>>,
    tls_required: AtomicBool,
    tls_ready: AtomicBool,
    tls_not_after_unix: AtomicI64,
    acme_last_success_unix: AtomicI64,
    acme_last_failure_unix: AtomicI64,
}

#[derive(Clone, Debug)]
struct DataDiskProbe {
    path: PathBuf,
    minimum_free_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct EdgeHealthSnapshot {
    pub tls: TlsHealthSnapshot,
    pub acme: AcmeHealthSnapshot,
    pub disk: Option<DiskHealthSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TlsHealthSnapshot {
    pub required: bool,
    pub ready: bool,
    pub certificate_not_after: Option<DateTime<Utc>>,
    pub certificate_expires_in_secs: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AcmeHealthSnapshot {
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiskHealthSnapshot {
    pub ready: bool,
    pub available_bytes: Option<u64>,
    pub minimum_free_bytes: u64,
    pub error: Option<String>,
}

static RECORDER: OnceLock<PrometheusHandle> = OnceLock::new();
static SERVER_START: OnceLock<Instant> = OnceLock::new();

/// Seconds since `Metrics::install` first ran for this process.
pub fn uptime_secs() -> u64 {
    SERVER_START
        .get()
        .map(|t| t.elapsed().as_secs())
        .unwrap_or(0)
}

impl Metrics {
    /// Install the Prometheus recorder (once per process) and return a
    /// handle bound to the supplied connection pool. The `version`
    /// label is recorded on the `sbol_db_build_info` gauge.
    ///
    /// To enable the worker / queue gauges, call
    /// [`Metrics::with_worker_pool`] and [`Metrics::with_jobs_repo`]
    /// before publishing the `AppState`.
    pub fn install(pool: Option<PgPool>, version: &'static str) -> Arc<Self> {
        let _ = SERVER_START.get_or_init(Instant::now);
        let handle = RECORDER
            .get_or_init(|| {
                PrometheusBuilder::new()
                    .set_buckets_for_metric(
                        Matcher::Full("http_request_duration_seconds".to_string()),
                        HTTP_DURATION_BUCKETS_SECONDS,
                    )
                    .expect("http histogram bucket config")
                    .set_buckets_for_metric(
                        Matcher::Full("sbol_db_jobs_duration_seconds".to_string()),
                        JOB_DURATION_BUCKETS_SECONDS,
                    )
                    .expect("job histogram bucket config")
                    .set_buckets_for_metric(
                        Matcher::Full("sbol_db_jobs_wait_seconds".to_string()),
                        JOB_WAIT_BUCKETS_SECONDS,
                    )
                    .expect("job wait histogram bucket config")
                    .install_recorder()
                    .expect("install prometheus recorder")
            })
            .clone();
        metrics::gauge!("sbol_db_build_info", "version" => version).set(1.0);
        for gauge in [
            "sbol_db_backup_last_success_timestamp_seconds",
            "sbol_db_backup_last_failure_timestamp_seconds",
            "sbol_db_backup_last_remote_verification_timestamp_seconds",
            "sbol_db_backup_scheduler_last_enqueue_timestamp_seconds",
            "sbol_db_backup_scheduler_next_timestamp_seconds",
            "sbol_db_backup_scheduler_interval_seconds",
            "sbol_db_backup_local_artifacts",
        ] {
            metrics::gauge!(gauge).set(0.0);
        }
        Arc::new(Self {
            handle,
            api_pool: pool,
            worker_pool: std::sync::Mutex::new(None),
            jobs: std::sync::Mutex::new(None),
            data_disk: std::sync::Mutex::new(None),
            tls_required: AtomicBool::new(false),
            tls_ready: AtomicBool::new(false),
            tls_not_after_unix: AtomicI64::new(0),
            acme_last_success_unix: AtomicI64::new(0),
            acme_last_failure_unix: AtomicI64::new(0),
        })
    }

    /// Require a deployed TLS certificate before `/readyz` reports success.
    /// Called before the operations router is published for an ACME listener.
    pub fn require_tls(&self) {
        self.tls_required.store(true, Ordering::Release);
        self.tls_ready.store(false, Ordering::Release);
        self.tls_not_after_unix.store(0, Ordering::Release);
        metrics::gauge!("sbol_db_tls_certificate_ready").set(0.0);
    }

    /// Mark the current ACME certificate as installed in rustls. Both a valid
    /// cached certificate and a newly issued certificate satisfy readiness.
    pub fn mark_tls_ready(&self, not_after_unix: i64) {
        self.tls_not_after_unix
            .store(not_after_unix, Ordering::Release);
        self.tls_ready.store(true, Ordering::Release);
        self.snapshot_tls();
    }

    pub fn record_acme_event(&self, success: bool) {
        let target = if success {
            &self.acme_last_success_unix
        } else {
            &self.acme_last_failure_unix
        };
        target.store(Utc::now().timestamp(), Ordering::Release);
    }

    pub fn edge_health_snapshot(&self) -> EdgeHealthSnapshot {
        let now = Utc::now().timestamp();
        let tls_required = self.tls_required.load(Ordering::Acquire);
        let not_after_unix = self.tls_not_after_unix.load(Ordering::Acquire);
        let certificate_not_after = timestamp(not_after_unix);
        let disk = self
            .data_disk
            .lock()
            .map(|probe| probe.clone())
            .ok()
            .flatten()
            .map(|probe| match fs2::available_space(&probe.path) {
                Ok(available) => DiskHealthSnapshot {
                    ready: available >= probe.minimum_free_bytes,
                    available_bytes: Some(available),
                    minimum_free_bytes: probe.minimum_free_bytes,
                    error: None,
                },
                Err(error) => DiskHealthSnapshot {
                    ready: false,
                    available_bytes: None,
                    minimum_free_bytes: probe.minimum_free_bytes,
                    error: Some(error.to_string()),
                },
            });
        EdgeHealthSnapshot {
            tls: TlsHealthSnapshot {
                required: tls_required,
                ready: self.tls_ready_for_traffic(),
                certificate_not_after,
                certificate_expires_in_secs: tls_required
                    .then_some(not_after_unix.saturating_sub(now).max(0)),
            },
            acme: AcmeHealthSnapshot {
                last_success_at: timestamp(self.acme_last_success_unix.load(Ordering::Acquire)),
                last_failure_at: timestamp(self.acme_last_failure_unix.load(Ordering::Acquire)),
            },
            disk,
        }
    }

    /// Whether the process may receive public traffic under its TLS policy.
    pub fn tls_ready_for_traffic(&self) -> bool {
        !self.tls_required.load(Ordering::Acquire)
            || (self.tls_ready.load(Ordering::Acquire)
                && self.tls_not_after_unix.load(Ordering::Acquire) > Utc::now().timestamp())
    }

    fn snapshot_tls(&self) {
        if !self.tls_required.load(Ordering::Acquire) {
            return;
        }
        let not_after = self.tls_not_after_unix.load(Ordering::Acquire);
        let remaining = not_after.saturating_sub(Utc::now().timestamp()).max(0);
        metrics::gauge!("sbol_db_tls_certificate_ready").set(if self.tls_ready_for_traffic() {
            1.0
        } else {
            0.0
        });
        metrics::gauge!("sbol_db_tls_certificate_not_after_timestamp_seconds")
            .set(not_after.max(0) as f64);
        metrics::gauge!("sbol_db_tls_certificate_expires_in_seconds").set(remaining as f64);
    }

    /// Attach the worker connection pool. Enables the
    /// `sbol_db_worker_pool_connections{state}` gauges.
    pub fn with_worker_pool(self: &Arc<Self>, pool: PgPool) -> Arc<Self> {
        *self.worker_pool.lock().expect("worker_pool poisoned") = Some(pool);
        self.clone()
    }

    /// Attach the job repository. Enables the queue-depth and
    /// oldest-queued-age gauges scraped at /metrics call time.
    pub fn with_jobs_repo(self: &Arc<Self>, jobs: Arc<dyn JobQueue>) -> Arc<Self> {
        *self.jobs.lock().expect("jobs repo poisoned") = Some(jobs);
        self.clone()
    }

    /// Attach the managed production filesystem to readiness and Prometheus
    /// snapshots. A scrape never mutates the filesystem.
    pub fn with_data_disk(self: &Arc<Self>, path: PathBuf, minimum_free_bytes: u64) -> Arc<Self> {
        *self.data_disk.lock().expect("data_disk poisoned") = Some(DataDiskProbe {
            path,
            minimum_free_bytes,
        });
        self.clone()
    }

    pub fn data_disk_ready_for_traffic(&self) -> Result<(), String> {
        let probe = self
            .data_disk
            .lock()
            .map_err(|_| "data disk probe lock is poisoned".to_owned())?
            .clone();
        let Some(probe) = probe else {
            return Ok(());
        };
        let available = fs2::available_space(&probe.path).map_err(|error| {
            format!(
                "cannot read available space for {}: {error}",
                probe.path.display()
            )
        })?;
        if available < probe.minimum_free_bytes {
            return Err(format!(
                "managed data filesystem is below its free-space reserve: available={available}, required={}",
                probe.minimum_free_bytes
            ));
        }
        Ok(())
    }

    fn snapshot_data_disk(&self) {
        let probe = self.data_disk.lock().expect("data_disk poisoned").clone();
        let Some(probe) = probe else {
            return;
        };
        metrics::gauge!("sbol_db_data_disk_minimum_free_bytes")
            .set(probe.minimum_free_bytes as f64);
        match fs2::available_space(&probe.path) {
            Ok(available) => {
                metrics::gauge!("sbol_db_data_disk_available_bytes").set(available as f64);
                metrics::gauge!("sbol_db_data_disk_ready").set(
                    if available >= probe.minimum_free_bytes {
                        1.0
                    } else {
                        0.0
                    },
                );
            }
            Err(error) => {
                tracing::warn!(
                    path = %probe.path.display(),
                    %error,
                    "data disk free-space snapshot failed"
                );
                metrics::gauge!("sbol_db_data_disk_ready").set(0.0);
                metrics::counter!("sbol_db_data_disk_probe_errors_total").increment(1);
            }
        }
    }

    fn snapshot_pool(label: &'static str, pool: &PgPool) {
        let size = pool.size() as f64;
        let idle = pool.num_idle() as f64;
        metrics::gauge!(format!("{label}_pool_connections"), "state" => "open").set(size);
        metrics::gauge!(format!("{label}_pool_connections"), "state" => "idle").set(idle);
        metrics::gauge!(format!("{label}_pool_connections"), "state" => "in_use")
            .set((size - idle).max(0.0));
    }

    /// In-memory snapshot of the API + worker pool capacity, suitable
    /// for direct JSON serialisation. Used by the lab observability
    /// summary handler; does no DB I/O.
    pub fn pool_snapshot(&self) -> PoolSnapshot {
        let api = self.api_pool.as_ref().map(Self::pool_stat);
        let worker = self
            .worker_pool
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(Self::pool_stat));
        PoolSnapshot { api, worker }
    }

    fn pool_stat(pool: &PgPool) -> PoolStat {
        let size = pool.size();
        let idle = pool.num_idle() as u32;
        PoolStat {
            size,
            idle,
            in_use: size.saturating_sub(idle),
        }
    }

    async fn snapshot_jobs(&self) {
        let jobs = {
            let lock = self.jobs.lock().expect("jobs poisoned");
            lock.clone()
        };
        let Some(jobs) = jobs else {
            return;
        };

        match jobs.queue_depth_snapshot().await {
            Ok(rows) => {
                for row in rows {
                    metrics::gauge!(
                        "sbol_db_jobs_queue_depth",
                        "status" => row.status.as_db_str(),
                        "queue" => row.queue,
                    )
                    .set(row.count as f64);
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "queue depth snapshot failed");
                metrics::counter!("sbol_db_jobs_scrape_errors_total", "scope" => "queue_depth")
                    .increment(1);
            }
        }

        match jobs.oldest_queued_age().await {
            Ok(rows) => {
                for row in rows {
                    metrics::gauge!(
                        "sbol_db_jobs_oldest_queued_age_seconds",
                        "queue" => row.queue,
                    )
                    .set(row.age_secs);
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "oldest queued age snapshot failed");
                metrics::counter!("sbol_db_jobs_scrape_errors_total", "scope" => "oldest_age")
                    .increment(1);
            }
        }

        // Constant -- always emit so dashboards have a known set of
        // labels to query against. `sbol_db_jobs_known_statuses` ensures
        // `sum by (status)` queries on queue_depth aren't blank when a
        // status is currently absent.
        for status in [
            JobStatus::Queued,
            JobStatus::Running,
            JobStatus::Succeeded,
            JobStatus::Failed,
            JobStatus::Cancelled,
            JobStatus::Dead,
        ] {
            metrics::gauge!(
                "sbol_db_jobs_status_enum",
                "status" => status.as_db_str(),
            )
            .set(1.0);
        }
    }

    async fn render(&self) -> String {
        self.snapshot_tls();
        self.snapshot_data_disk();
        if let Some(pool) = self.api_pool.as_ref() {
            Self::snapshot_pool("sbol_db", pool);
        }
        let worker_pool = {
            let lock = self.worker_pool.lock().expect("worker pool poisoned");
            lock.clone()
        };
        if let Some(pool) = worker_pool.as_ref() {
            Self::snapshot_pool("sbol_db_worker", pool);
        }
        self.snapshot_jobs().await;
        self.handle.render()
    }
}

fn timestamp(value: i64) -> Option<DateTime<Utc>> {
    (value > 0)
        .then(|| DateTime::from_timestamp(value, 0))
        .flatten()
}

pub async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        state.metrics.render().await,
    )
}

#[derive(Clone, Debug, Serialize)]
pub struct PoolSnapshot {
    pub api: Option<PoolStat>,
    pub worker: Option<PoolStat>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PoolStat {
    pub size: u32,
    pub idle: u32,
    pub in_use: u32,
}

pub async fn track_metrics(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m: &MatchedPath| m.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".to_owned());
    let start = Instant::now();
    let response = next.run(req).await;
    let elapsed = start.elapsed();
    let status_code = response.status().as_u16();
    let status = status_code.to_string();
    let labels = [
        ("method", method.as_str().to_owned()),
        ("route", route.clone()),
        ("status", status),
    ];
    metrics::counter!("http_requests_total", &labels).increment(1);
    metrics::histogram!("http_request_duration_seconds", &labels).record(elapsed.as_secs_f64());

    // Feed the in-process rolling window used by the lab observability
    // page. Skip the noisy operational routes that would otherwise
    // dominate the chart in an otherwise-idle deployment.
    if rolling_should_record(&route) {
        rolling().record(elapsed, status_code);
    }

    response
}

/// Dedicated bounded-cardinality counter for the optional SBOLExplorer
/// listener. The main and compatibility routers share the process-global HTTP
/// counters, so this additional metric lets integration tests prove a search
/// actually reached the compatibility listener rather than a fallback route.
pub async fn track_explorer_metrics(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".to_owned());
    let response = next.run(req).await;
    metrics::counter!(
        "sbol_db_explorer_requests_total",
        "method" => method.as_str().to_owned(),
        "route" => route,
        "status" => response.status().as_u16().to_string(),
    )
    .increment(1);
    response
}

/// Record use of the SynBioHub V1 compatibility adapter without retaining a
/// caller-supplied path, object identity, username, query, or request body.
///
/// The `family` label is selected from a fixed vocabulary. In particular, the
/// normal HTTP route label is deliberately not reused here: even a templated
/// compatibility route can contain a wildcard search expression, while the
/// retirement signal only needs to answer which workflow families remain in
/// use.
pub async fn track_synbiohub_v1_usage(req: Request, next: Next) -> Response {
    track_compatibility_usage(req, next, "synbiohub_v1").await
}

/// Record use of the Virtuoso-compatible graph protocol separately from the
/// SynBioHub application adapter. It follows the same privacy and cardinality
/// contract as [`track_synbiohub_v1_usage`].
pub async fn track_virtuoso_compatibility_usage(req: Request, next: Next) -> Response {
    track_compatibility_usage(req, next, "virtuoso_protocol").await
}

async fn track_compatibility_usage(req: Request, next: Next, surface: &'static str) -> Response {
    let method = req.method().clone();
    let path = req
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or_else(|| req.uri().path());
    let family = compatibility_family(path);
    let response = next.run(req).await;
    metrics::counter!(
        "sbol_db_compatibility_requests_total",
        "surface" => surface,
        "family" => family,
        "method" => method.as_str().to_owned(),
        "status" => response.status().as_u16().to_string(),
    )
    .increment(1);
    response
}

/// Count transitional `/lab` page bookmarks without exporting the deep-link
/// value. API calls and static assets are excluded; the metric represents only
/// browser navigations that can be migrated to `/admin`.
pub async fn track_legacy_ui_usage(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let bookmark = legacy_ui_bookmark(&req);
    let response = next.run(req).await;
    if let Some(bookmark) = bookmark {
        metrics::counter!(
            "sbol_db_legacy_ui_requests_total",
            "bookmark" => bookmark,
            "method" => method.as_str().to_owned(),
            "status" => response.status().as_u16().to_string(),
        )
        .increment(1);
    }
    response
}

fn compatibility_family(path: &str) -> &'static str {
    if path.starts_with("/sparql-auth") || path.starts_with("/sparql-graph-crud-auth") {
        return "graph_protocol";
    }
    if path == "/login"
        || path == "/logout"
        || path == "/register"
        || path == "/profile"
        || path == "/resetPassword"
        || path == "/setNewPassword"
        || path == "/setup"
    {
        return "identity";
    }
    if path == "/admin" || path.starts_with("/admin/") {
        return "administration";
    }
    if path.starts_with("/remote")
        || path == "/updateWebOfRegistries"
        || path.ends_with("/copyFromRemote")
    {
        return "federation";
    }
    if path == "/callPlugin" || path.starts_with("/expose/") || path.starts_with("/stream/") {
        return "plugins";
    }
    if path.starts_with("/actions/job/") || path == "/corruptLog" {
        return "jobs";
    }
    if path == "/search"
        || path.starts_with("/search/")
        || path == "/searchCount"
        || path.starts_with("/searchCount/")
        || path.ends_with("/count")
        || path == "/rootCollections"
        || path == "/browse"
        || path.starts_with("/autocomplete/")
        || path == "/api/datatables"
        || path.starts_with("/api/stream/")
        || path == "/sbsearch"
    {
        return "discovery";
    }
    if path == "/submit" || path == "/submit/" {
        return "submission";
    }
    if path == "/manage" || path == "/shared" {
        return "workspace";
    }
    if path.contains("/share") || path.ends_with("/shareLink") {
        return "sharing";
    }
    if path.ends_with("/attach") || path.ends_with("/attachUrl") || path.ends_with("/download") {
        return "attachments";
    }
    if [
        "/sbol", "/sbolnr", "/gb", "/fasta", "/gff", "/omex", "/summary", "/full", "/icon",
    ]
    .iter()
    .any(|suffix| path.ends_with(suffix))
    {
        return "representations";
    }
    if [
        "/metadata",
        "/uses",
        "/usesCount",
        "/similar",
        "/similarCount",
        "/twins",
        "/twinsCount",
        "/subCollections",
    ]
    .iter()
    .any(|suffix| path.ends_with(suffix))
    {
        return "object_reads";
    }
    if path.starts_with("/updateMutable")
        || path == "/updateCitations"
        || [
            "/remove",
            "/replace",
            "/removeCollection",
            "/removeMembership",
            "/addToCollection",
            "/addOwner",
            "/makePublic",
        ]
        .iter()
        .any(|suffix| path.ends_with(suffix))
        || path.contains("/edit/")
        || path.contains("/add/")
        || path.contains("/remove/")
        || path.contains("/removeOwner/")
    {
        return "object_mutations";
    }
    "other"
}

fn legacy_ui_bookmark(req: &Request) -> Option<&'static str> {
    if req.method() != axum::http::Method::GET && req.method() != axum::http::Method::HEAD {
        return None;
    }
    let path = req.uri().path();
    if path == "/lab" || path == "/lab/" {
        return accepts_html(req).then_some("root");
    }
    if !path.starts_with("/lab/")
        || path.starts_with("/lab/api/")
        || path == "/lab/api"
        || path.starts_with("/lab/assets/")
    {
        return None;
    }
    accepts_html(req).then_some("deep_link")
}

fn accepts_html(req: &Request) -> bool {
    req.headers()
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|item| item.trim().starts_with("text/html"))
        })
}

// ---------- Rolling in-process traffic stats (powers /lab/api/observability/summary)

/// Width of each rolling bucket, in seconds.
const ROLLING_BUCKET_SECS: u64 = 10;
/// Total number of buckets retained — `WINDOW_BUCKETS * BUCKET_SECS` seconds.
const ROLLING_WINDOW_BUCKETS: usize = 60;
/// Cap on per-bucket latency samples retained for quantile estimation.
const ROLLING_SAMPLE_CAP: usize = 256;

static ROLLING: OnceLock<RollingStats> = OnceLock::new();

fn rolling() -> &'static RollingStats {
    ROLLING.get_or_init(RollingStats::new)
}

fn rolling_should_record(route: &str) -> bool {
    !matches!(
        route,
        "/healthz" | "/readyz" | "/metrics" | "/docs" | "/openapi.json" | "unmatched"
    ) && !route.starts_with("/lab")
}

struct RollingStats {
    inner: Mutex<RollingInner>,
}

struct RollingInner {
    buckets: VecDeque<Bucket>,
}

struct Bucket {
    /// Bucket start as both monotonic (rollover decisions) and wall-clock
    /// (JSON output). The two never drift by more than a few microseconds
    /// because they're sampled in the same statement.
    started_mono: Instant,
    started_wall: SystemTime,
    count: u64,
    error_count: u64,
    samples_ms: Vec<f32>,
    seen: u64,
    max_ms: f32,
}

impl RollingStats {
    fn new() -> Self {
        Self {
            inner: Mutex::new(RollingInner {
                buckets: VecDeque::with_capacity(ROLLING_WINDOW_BUCKETS),
            }),
        }
    }

    fn record(&self, elapsed: Duration, status_code: u16) {
        let now_mono = Instant::now();
        let now_wall = SystemTime::now();
        let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
        let is_error = status_code >= 500;
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let bucket_width = Duration::from_secs(ROLLING_BUCKET_SECS);

        // Drop fully-aged-out buckets so the deque never grows past N.
        let window = Duration::from_secs(ROLLING_BUCKET_SECS * ROLLING_WINDOW_BUCKETS as u64);
        while let Some(front) = inner.buckets.front() {
            if now_mono.duration_since(front.started_mono) > window {
                inner.buckets.pop_front();
            } else {
                break;
            }
        }

        // Open new bucket(s) if we've rolled past the back-most one's edge.
        let mut needs_new = match inner.buckets.back() {
            Some(b) => now_mono.duration_since(b.started_mono) >= bucket_width,
            None => true,
        };
        while needs_new {
            inner.buckets.push_back(Bucket {
                started_mono: now_mono,
                started_wall: now_wall,
                count: 0,
                error_count: 0,
                samples_ms: Vec::with_capacity(8),
                seen: 0,
                max_ms: 0.0,
            });
            if inner.buckets.len() > ROLLING_WINDOW_BUCKETS {
                inner.buckets.pop_front();
            }
            needs_new = false;
        }

        let back = inner.buckets.back_mut().expect("just pushed");
        back.count += 1;
        back.seen += 1;
        if is_error {
            back.error_count += 1;
        }
        let sample = elapsed_ms as f32;
        if sample > back.max_ms {
            back.max_ms = sample;
        }
        if back.samples_ms.len() < ROLLING_SAMPLE_CAP {
            back.samples_ms.push(sample);
        } else {
            // Reservoir sampling: each subsequent observation replaces a
            // random slot with decreasing probability so the kept set
            // stays a uniform sample of the bucket's true latency
            // distribution.
            let seen = back.seen;
            let slot = (fast_rand(seen) % seen) as usize;
            if slot < ROLLING_SAMPLE_CAP {
                back.samples_ms[slot] = sample;
            }
        }
    }

    fn snapshot(&self) -> RollingSnapshot {
        let Ok(inner) = self.inner.lock() else {
            return RollingSnapshot::empty();
        };
        let buckets = inner
            .buckets
            .iter()
            .map(|b| {
                let (p50, p95, p99) = percentiles(&b.samples_ms);
                BucketSnapshot {
                    started_at: DateTime::<Utc>::from(b.started_wall),
                    count: b.count,
                    error_count: b.error_count,
                    p50_ms: p50,
                    p95_ms: p95,
                    p99_ms: p99,
                    max_ms: b.max_ms as f64,
                }
            })
            .collect();
        RollingSnapshot {
            bucket_secs: ROLLING_BUCKET_SECS,
            window_buckets: ROLLING_WINDOW_BUCKETS,
            buckets,
        }
    }
}

/// xorshift32 — cheap PRNG good enough for reservoir slot selection.
fn fast_rand(seed: u64) -> u64 {
    let mut x = seed
        .wrapping_mul(0x9E3779B97F4A7C15)
        .wrapping_add(0xD1B54A32D192ED03);
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51AFD7ED558CCD);
    x ^= x >> 33;
    x
}

fn percentiles(samples: &[f32]) -> (f64, f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mut sorted: Vec<f32> = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pick = |q: f64| -> f64 {
        let idx = ((sorted.len() as f64 - 1.0) * q).round() as usize;
        sorted[idx.min(sorted.len() - 1)] as f64
    };
    (pick(0.50), pick(0.95), pick(0.99))
}

#[derive(Clone, Debug, Serialize)]
pub struct RollingSnapshot {
    pub bucket_secs: u64,
    pub window_buckets: usize,
    pub buckets: Vec<BucketSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BucketSnapshot {
    pub started_at: DateTime<Utc>,
    pub count: u64,
    pub error_count: u64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

impl RollingSnapshot {
    fn empty() -> Self {
        Self {
            bucket_secs: ROLLING_BUCKET_SECS,
            window_buckets: ROLLING_WINDOW_BUCKETS,
            buckets: Vec::new(),
        }
    }
}

/// Read the rolling traffic snapshot. Called by the lab observability
/// summary handler.
pub fn rolling_snapshot() -> RollingSnapshot {
    rolling().snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_disk_reserve_participates_in_readiness() {
        let temp = tempfile::tempdir().unwrap();
        let metrics =
            Metrics::install(None, "disk-test").with_data_disk(temp.path().to_path_buf(), u64::MAX);
        assert!(metrics.data_disk_ready_for_traffic().is_err());

        let metrics = metrics.with_data_disk(temp.path().to_path_buf(), 0);
        assert!(metrics.data_disk_ready_for_traffic().is_ok());
    }
}
