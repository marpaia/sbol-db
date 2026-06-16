//! Prometheus metrics surface for the HTTP server.
//!
//! A single process-global recorder is installed on first call to
//! [`Metrics::install`]; subsequent calls reuse the same handle so unit
//! tests that build multiple routers don't panic on the second install.
//! Cardinality is bounded: HTTP labels use `axum::extract::MatchedPath`
//! to template the route (e.g. `/objects/:id`, not the raw IRI), and
//! requests that didn't match a route are bucketed as `unmatched`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use axum::extract::{MatchedPath, Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use sbol_db_postgres::{JobRepository, PgPool};
use sbol_db_storage::JobStatus;
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
    api_pool: PgPool,
    worker_pool: std::sync::Mutex<Option<PgPool>>,
    jobs: std::sync::Mutex<Option<Arc<JobRepository>>>,
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
    pub fn install(pool: PgPool, version: &'static str) -> Arc<Self> {
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
        Arc::new(Self {
            handle,
            api_pool: pool,
            worker_pool: std::sync::Mutex::new(None),
            jobs: std::sync::Mutex::new(None),
        })
    }

    /// Attach the worker connection pool. Enables the
    /// `sbol_db_worker_pool_connections{state}` gauges.
    pub fn with_worker_pool(self: &Arc<Self>, pool: PgPool) -> Arc<Self> {
        *self.worker_pool.lock().expect("worker_pool poisoned") = Some(pool);
        self.clone()
    }

    /// Attach the job repository. Enables the queue-depth and
    /// oldest-queued-age gauges scraped at /metrics call time.
    pub fn with_jobs_repo(self: &Arc<Self>, jobs: Arc<JobRepository>) -> Arc<Self> {
        *self.jobs.lock().expect("jobs repo poisoned") = Some(jobs);
        self.clone()
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
        let api = Self::pool_stat(&self.api_pool);
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
        Self::snapshot_pool("sbol_db", &self.api_pool);
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

pub async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        state.metrics.render().await,
    )
}

#[derive(Clone, Debug, Serialize)]
pub struct PoolSnapshot {
    pub api: PoolStat,
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
