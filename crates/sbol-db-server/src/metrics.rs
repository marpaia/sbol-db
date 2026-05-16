//! Prometheus metrics surface for the HTTP server.
//!
//! A single process-global recorder is installed on first call to
//! [`Metrics::install`]; subsequent calls reuse the same handle so unit
//! tests that build multiple routers don't panic on the second install.
//! Cardinality is bounded: HTTP labels use `axum::extract::MatchedPath`
//! to template the route (e.g. `/objects/:id`, not the raw IRI), and
//! requests that didn't match a route are bucketed as `unmatched`.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use axum::extract::{MatchedPath, Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use sbol_db_postgres::{JobRepository, JobStatus, PgPool};

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

impl Metrics {
    /// Install the Prometheus recorder (once per process) and return a
    /// handle bound to the supplied connection pool. The `version`
    /// label is recorded on the `sbol_db_build_info` gauge.
    ///
    /// To enable the worker / queue gauges, call
    /// [`Metrics::with_worker_pool`] and [`Metrics::with_jobs_repo`]
    /// before publishing the `AppState`.
    pub fn install(pool: PgPool, version: &'static str) -> Arc<Self> {
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

pub async fn track_metrics(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m: &MatchedPath| m.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".to_owned());
    let start = Instant::now();
    let response = next.run(req).await;
    let elapsed = start.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();
    let labels = [
        ("method", method.as_str().to_owned()),
        ("route", route),
        ("status", status),
    ];
    metrics::counter!("http_requests_total", &labels).increment(1);
    metrics::histogram!("http_request_duration_seconds", &labels).record(elapsed);
    response
}
