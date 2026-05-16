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
use sbol_db_postgres::PgPool;

use crate::AppState;

const HTTP_DURATION_BUCKETS_SECONDS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

pub struct Metrics {
    handle: PrometheusHandle,
    pool: PgPool,
}

static RECORDER: OnceLock<PrometheusHandle> = OnceLock::new();

impl Metrics {
    /// Install the Prometheus recorder (once per process) and return a
    /// handle bound to the supplied connection pool. The `version`
    /// label is recorded on the `sbol_db_build_info` gauge.
    pub fn install(pool: PgPool, version: &'static str) -> Arc<Self> {
        let handle = RECORDER
            .get_or_init(|| {
                PrometheusBuilder::new()
                    .set_buckets_for_metric(
                        Matcher::Full("http_request_duration_seconds".to_string()),
                        HTTP_DURATION_BUCKETS_SECONDS,
                    )
                    .expect("histogram bucket config")
                    .install_recorder()
                    .expect("install prometheus recorder")
            })
            .clone();
        metrics::gauge!("sbol_db_build_info", "version" => version).set(1.0);
        Arc::new(Self { handle, pool })
    }

    fn render(&self) -> String {
        let size = self.pool.size() as f64;
        let idle = self.pool.num_idle() as f64;
        metrics::gauge!("sbol_db_pool_connections", "state" => "open").set(size);
        metrics::gauge!("sbol_db_pool_connections", "state" => "idle").set(idle);
        metrics::gauge!("sbol_db_pool_connections", "state" => "in_use")
            .set((size - idle).max(0.0));
        self.handle.render()
    }
}

pub async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        state.metrics.render(),
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
