use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use sbol_db_core::{DomainError, JobId};
use sbol_db_postgres::{JobRepository, SbolJob, SbolObjectService, DEFAULT_QUEUE};
use sqlx::postgres::PgListener;
use sqlx::PgPool;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::context::JobContext;
use crate::handler::HandlerError;
use crate::registry::JobRegistry;

/// How often the worker writes a fresh heartbeat timestamp. Operators
/// alert on `time() - sbol_db_worker_heartbeat_timestamp_seconds` to
/// detect stuck or dead workers.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// Tunables for a single worker process. All fields have defaults that
/// suit the embedded-in-`sbol-db serve` case; tweak only when running a
/// dedicated worker fleet.
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    /// Worker identity. Persisted as `sbol_jobs.leased_by` so operators
    /// can attribute work. Defaults to `<hostname>-<pid>-<random>`.
    pub worker_id: Arc<str>,
    /// Queues this worker subscribes to. Empty means [`DEFAULT_QUEUE`].
    pub queues: Vec<String>,
    /// Maximum concurrent in-flight handler tasks.
    pub concurrency: usize,
    /// Lease duration handed out on dequeue. The renewer extends it
    /// while the job is still running.
    pub lease: Duration,
    /// How often the renewer extends a held lease. Should be safely
    /// below `lease`; defaults to `lease / 3`.
    pub renew_interval: Duration,
    /// How often the reaper looks for expired leases.
    pub reaper_interval: Duration,
    /// Maximum interval the dequeue loop waits between polls when no
    /// `LISTEN/NOTIFY` wake-up arrives. Reduces tail latency when
    /// NOTIFY is unavailable.
    pub poll_interval: Duration,
    /// Grace period for in-flight handlers during shutdown. Past this,
    /// the worker stops awaiting; the lease-expiry path covers the rest.
    pub shutdown_grace: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let lease = Duration::from_secs(60);
        Self {
            worker_id: default_worker_id().into(),
            queues: vec![DEFAULT_QUEUE.to_owned()],
            concurrency: cpus,
            lease,
            renew_interval: lease / 3,
            reaper_interval: Duration::from_secs(30),
            poll_interval: Duration::from_secs(5),
            shutdown_grace: Duration::from_secs(30),
        }
    }
}

fn default_worker_id() -> String {
    let host = hostname().unwrap_or_else(|| "unknown".to_owned());
    let pid = std::process::id();
    let suffix = uuid_short();
    format!("{host}-{pid}-{suffix}")
}

fn hostname() -> Option<String> {
    // Avoid an extra crate dependency. POSIX `HOSTNAME` is good enough
    // for log attribution; nodes that want a stable id should pass it
    // explicitly via WorkerConfig.
    std::env::var("HOSTNAME").ok().or_else(|| {
        std::process::Command::new("hostname")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    })
}

fn uuid_short() -> String {
    let id = uuid::Uuid::new_v4().to_string();
    id.split('-').next().unwrap_or(&id).to_owned()
}

/// One worker. Owns the dequeue loop, the lease renewer, the reaper,
/// and the LISTEN/NOTIFY task. Lives as long as its [`CancellationToken`]
/// stays alive; `run` returns once shutdown has drained or timed out.
pub struct Worker {
    pool: PgPool,
    repo: JobRepository,
    service: Arc<SbolObjectService>,
    registry: Arc<JobRegistry>,
    config: WorkerConfig,
}

impl Worker {
    pub fn new(
        pool: PgPool,
        service: Arc<SbolObjectService>,
        registry: Arc<JobRegistry>,
        config: WorkerConfig,
    ) -> Self {
        let repo = JobRepository::new(pool.clone());
        Self {
            pool,
            repo,
            service,
            registry,
            config,
        }
    }

    /// Run until `cancel` fires, then drain in-flight handlers up to the
    /// configured grace window.
    pub async fn run(self, cancel: CancellationToken) -> Result<(), DomainError> {
        tracing::info!(
            worker_id = %self.config.worker_id,
            concurrency = self.config.concurrency,
            queues = ?self.config.queues,
            kinds = self.registry.len(),
            "starting sbol-db worker",
        );

        // Identity / static gauges. Bounded cardinality (one series per
        // worker process); the heartbeat lets ops alert on workers that
        // stop ticking even when they aren't taking work.
        let worker_id = self.config.worker_id.clone();
        metrics::gauge!(
            "sbol_db_worker_concurrency",
            "worker_id" => worker_id.to_string()
        )
        .set(self.config.concurrency as f64);
        record_heartbeat(&worker_id);

        let semaphore = Arc::new(Semaphore::new(self.config.concurrency.max(1)));
        let mut in_flight: JoinSet<()> = JoinSet::new();

        let reaper_handle = spawn_reaper(self.repo.clone(), self.config.clone(), cancel.clone());
        let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let listen_handle = spawn_listener(
            self.pool.clone(),
            self.config.queues.clone(),
            notify_tx,
            cancel.clone(),
        );
        let heartbeat_handle = spawn_heartbeat(worker_id.clone(), cancel.clone());

        loop {
            if cancel.is_cancelled() {
                break;
            }

            // Block until a permit is available, or shutdown fires.
            let permit = tokio::select! {
                _ = cancel.cancelled() => break,
                permit = semaphore.clone().acquire_owned() => permit
                    .expect("worker semaphore closed unexpectedly"),
            };

            let job = match self
                .repo
                .dequeue(
                    &self.config.queues,
                    &self.config.worker_id,
                    self.config.lease,
                )
                .await
            {
                Ok(Some(j)) => j,
                Ok(None) => {
                    // Nothing to do. Release the permit; sleep until a
                    // NOTIFY arrives, the poll interval elapses, or
                    // shutdown is requested.
                    drop(permit);
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = notify_rx.recv() => continue,
                        _ = tokio::time::sleep(self.config.poll_interval) => continue,
                    }
                }
                Err(err) => {
                    tracing::error!(error = %err, "dequeue failed; backing off");
                    metrics::counter!("sbol_db_jobs_dequeue_errors_total").increment(1);
                    drop(permit);
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(self.config.poll_interval) => continue,
                    }
                }
            };

            let job_kind = job.kind.clone();
            let job_queue = job.queue.clone();

            // Wait time (first attempt only — retries already paid this
            // bucket once and have their own backoff arithmetic).
            if job.attempts == 1 {
                let wait_secs = (chrono::Utc::now() - job.created_at)
                    .num_milliseconds()
                    .max(0) as f64
                    / 1000.0;
                metrics::histogram!(
                    "sbol_db_jobs_wait_seconds",
                    "kind" => job_kind.clone(),
                    "queue" => job_queue.clone(),
                )
                .record(wait_secs);
            }

            metrics::counter!(
                "sbol_db_jobs_started_total",
                "kind" => job_kind.clone(),
                "queue" => job_queue.clone(),
                "worker_id" => worker_id.to_string(),
            )
            .increment(1);
            metrics::gauge!(
                "sbol_db_worker_inflight",
                "worker_id" => worker_id.to_string(),
            )
            .increment(1.0);

            let handler = self.registry.lookup(&job_kind);
            let Some(handler) = handler else {
                tracing::error!(
                    job_id = %job.id,
                    kind = %job_kind,
                    "no handler registered for job kind; marking failed",
                );
                metrics::counter!(
                    "sbol_db_jobs_completed_total",
                    "kind" => job_kind.clone(),
                    "queue" => job_queue.clone(),
                    "status" => "handler_missing",
                )
                .increment(1);
                metrics::gauge!(
                    "sbol_db_worker_inflight",
                    "worker_id" => worker_id.to_string(),
                )
                .decrement(1.0);
                let _ = self
                    .repo
                    .mark_failed(
                        job.id,
                        &self.config.worker_id,
                        &format!("no handler registered for kind '{job_kind}'"),
                    )
                    .await;
                drop(permit);
                continue;
            };

            let ctx = JobContext {
                job_id: job.id,
                worker_id: self.config.worker_id.clone(),
                attempt: job.attempts,
                service: self.service.clone(),
                jobs: self.repo.clone(),
                cancel: cancel.clone(),
            };
            let repo = self.repo.clone();
            let cfg = self.config.clone();
            let worker_cancel = cancel.clone();
            let worker_id_for_task = worker_id.clone();

            in_flight.spawn(async move {
                run_one_job(handler, ctx, job, repo, cfg, worker_cancel).await;
                metrics::gauge!(
                    "sbol_db_worker_inflight",
                    "worker_id" => worker_id_for_task.to_string(),
                )
                .decrement(1.0);
                drop(permit);
            });

            // Reap any finished handler tasks without blocking. Keeps
            // memory bounded; panics are logged in run_one_job already.
            while let Some(res) = in_flight.try_join_next() {
                if let Err(err) = res {
                    tracing::error!(error = %err, "handler task panicked / aborted");
                }
            }
        }

        tracing::info!(
            in_flight = in_flight.len(),
            grace_secs = self.config.shutdown_grace.as_secs(),
            "shutdown requested; draining in-flight jobs",
        );

        let drain_deadline = tokio::time::Instant::now() + self.config.shutdown_grace;
        loop {
            if in_flight.is_empty() {
                break;
            }
            let join = tokio::select! {
                res = in_flight.join_next() => res,
                _ = tokio::time::sleep_until(drain_deadline) => {
                    tracing::warn!(
                        remaining = in_flight.len(),
                        "shutdown grace exhausted; abandoning in-flight handlers \
                         (leases will expire and another node will retry)",
                    );
                    in_flight.shutdown().await;
                    break;
                }
            };
            if let Some(Err(err)) = join {
                tracing::error!(error = %err, "handler task panicked during drain");
            }
        }

        let _ = tokio::join!(reaper_handle, listen_handle, heartbeat_handle);
        tracing::info!(worker_id = %self.config.worker_id, "worker shut down");
        Ok(())
    }
}

fn record_heartbeat(worker_id: &Arc<str>) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    metrics::gauge!(
        "sbol_db_worker_heartbeat_timestamp_seconds",
        "worker_id" => worker_id.to_string(),
    )
    .set(now);
}

fn spawn_heartbeat(worker_id: Arc<str>, cancel: CancellationToken) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            record_heartbeat(&worker_id);
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(HEARTBEAT_INTERVAL) => {}
            }
        }
    })
}

async fn run_one_job(
    handler: Arc<dyn crate::handler::ErasedHandler>,
    ctx: JobContext,
    job: SbolJob,
    repo: JobRepository,
    cfg: WorkerConfig,
    cancel: CancellationToken,
) {
    let job_id = job.id;
    let job_kind = job.kind.clone();
    let job_queue = job.queue.clone();
    let worker_id = cfg.worker_id.clone();
    let attempt = job.attempts;
    let payload = job.payload;

    // One span around the whole handler invocation, attached to this
    // task's async body via `.instrument`. Using `Span::enter()` on an
    // async fn nests sibling tasks' spans whenever tokio multiplexes
    // them onto the same thread; `Instrument` follows the task across
    // .await points and stays scoped to it.
    let span = tracing::info_span!(
        "job",
        job_id = %job_id,
        kind = %job_kind,
        attempt = attempt,
    );

    async move {
        let renew_cancel = CancellationToken::new();
        let renew_handle = spawn_renewer(
            repo.clone(),
            job_id,
            worker_id.clone(),
            cfg.lease,
            cfg.renew_interval,
            renew_cancel.clone(),
            cancel.clone(),
        );

        tracing::info!("job started");
        let started = Instant::now();
        append_job_log(
            &repo,
            job_id,
            Some(attempt),
            "info",
            "job started",
            serde_json::json!({
                "kind": job_kind.as_str(),
                "queue": job_queue.as_str(),
                "worker_id": worker_id.to_string(),
            }),
        )
        .await;
        let outcome = handler.dispatch(ctx, payload).await;
        let elapsed = started.elapsed().as_secs_f64();

        renew_cancel.cancel();
        let _ = renew_handle.await;

        match outcome {
            Ok(out) => {
                if let Err(err) = repo.mark_succeeded(job_id, &worker_id, out.result).await {
                    tracing::error!(error = %err, "failed to mark job succeeded");
                } else {
                    append_job_log(
                        &repo,
                        job_id,
                        Some(attempt),
                        "info",
                        "job succeeded",
                        serde_json::json!({ "elapsed_secs": elapsed }),
                    )
                    .await;
                    tracing::info!(elapsed_secs = elapsed, "job succeeded");
                }
                metrics::counter!(
                    "sbol_db_jobs_completed_total",
                    "kind" => job_kind.clone(),
                    "queue" => job_queue.clone(),
                    "status" => "succeeded",
                )
                .increment(1);
                metrics::histogram!(
                    "sbol_db_jobs_duration_seconds",
                    "kind" => job_kind,
                    "status" => "succeeded",
                )
                .record(elapsed);
            }
            Err(err) => {
                let msg = match err {
                    HandlerError::InvalidPayload(s) => format!("invalid payload: {s}"),
                    HandlerError::Domain(d) => d.to_string(),
                    HandlerError::Other(s) => s,
                };
                let terminal = match repo.mark_failed(job_id, &worker_id, &msg).await {
                    Ok(status) => {
                        append_job_log(
                            &repo,
                            job_id,
                            Some(attempt),
                            if status == sbol_db_postgres::JobStatus::Queued {
                                "warn"
                            } else {
                                "error"
                            },
                            if status == sbol_db_postgres::JobStatus::Queued {
                                "job failed; retry scheduled"
                            } else {
                                "job failed permanently"
                            },
                            serde_json::json!({
                                "elapsed_secs": elapsed,
                                "error": msg,
                                "next_status": status,
                            }),
                        )
                        .await;
                        tracing::warn!(error = %msg, next_status = ?status, elapsed_secs = elapsed, "job failed");
                        status
                    }
                    Err(e) => {
                        tracing::error!(error = %e, original = %msg, "failed to record job failure");
                        sbol_db_postgres::JobStatus::Failed
                    }
                };
                let status_label = match terminal {
                    sbol_db_postgres::JobStatus::Dead => "dead",
                    _ => "failed",
                };
                metrics::counter!(
                    "sbol_db_jobs_completed_total",
                    "kind" => job_kind.clone(),
                    "queue" => job_queue.clone(),
                    "status" => status_label,
                )
                .increment(1);
                metrics::histogram!(
                    "sbol_db_jobs_duration_seconds",
                    "kind" => job_kind,
                    "status" => status_label,
                )
                .record(elapsed);
            }
        }
    }
    .instrument(span)
    .await;
}

async fn append_job_log(
    repo: &JobRepository,
    job_id: JobId,
    attempt_no: Option<i32>,
    level: &str,
    message: &str,
    fields: serde_json::Value,
) {
    if let Err(err) = repo
        .append_log(job_id, attempt_no, level, message, fields)
        .await
    {
        tracing::warn!(error = %err, job_id = %job_id, "failed to append job log");
    }
}

fn spawn_renewer(
    repo: JobRepository,
    job_id: JobId,
    worker_id: Arc<str>,
    lease: Duration,
    interval: Duration,
    stop: CancellationToken,
    global: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = stop.cancelled() => break,
                _ = global.cancelled() => break,
                _ = tokio::time::sleep(interval) => {}
            }
            match repo.renew_lease(job_id, &worker_id, lease).await {
                Ok(true) => {
                    metrics::counter!(
                        "sbol_db_jobs_lease_renewals_total",
                        "result" => "ok",
                    )
                    .increment(1);
                }
                Ok(false) => {
                    tracing::warn!(%job_id, "lease lost (stolen or job no longer running)");
                    metrics::counter!(
                        "sbol_db_jobs_lease_renewals_total",
                        "result" => "lost",
                    )
                    .increment(1);
                    break;
                }
                Err(err) => {
                    tracing::warn!(error = %err, %job_id, "lease renewal failed");
                    metrics::counter!(
                        "sbol_db_jobs_lease_renewals_total",
                        "result" => "error",
                    )
                    .increment(1);
                }
            }
        }
    })
}

fn spawn_reaper(
    repo: JobRepository,
    cfg: WorkerConfig,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(cfg.reaper_interval) => {}
            }
            match repo.reap_expired_leases().await {
                Ok(0) => {}
                Ok(n) => {
                    tracing::warn!(reclaimed = n, "reaped expired job leases");
                    metrics::counter!("sbol_db_jobs_reaped_total").increment(n);
                }
                Err(err) => {
                    tracing::warn!(error = %err, "reap failed");
                    metrics::counter!("sbol_db_jobs_reap_errors_total").increment(1);
                }
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            metrics::gauge!("sbol_db_jobs_reaper_last_run_timestamp_seconds").set(now);
        }
    })
}

fn spawn_listener(
    pool: PgPool,
    queues: Vec<String>,
    notify: tokio::sync::mpsc::UnboundedSender<()>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Best-effort: if LISTEN fails we degrade to the poll-interval
        // fallback. Reconnect on transient errors.
        loop {
            if cancel.is_cancelled() {
                break;
            }
            let mut listener = match PgListener::connect_with(&pool).await {
                Ok(l) => l,
                Err(err) => {
                    tracing::warn!(error = %err, "job listener connect failed; falling back to polling");
                    metrics::counter!(
                        "sbol_db_jobs_listener_reconnects_total",
                        "reason" => "connect_failed",
                    )
                    .increment(1);
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                    }
                }
            };
            if let Err(err) = listener.listen("sbol_jobs_enqueued").await {
                tracing::warn!(error = %err, "LISTEN sbol_jobs_enqueued failed");
                metrics::counter!(
                    "sbol_db_jobs_listener_reconnects_total",
                    "reason" => "listen_failed",
                )
                .increment(1);
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                }
            }
            metrics::gauge!("sbol_db_jobs_listener_connected").set(1.0);

            let mut stream = listener.into_stream();
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        metrics::gauge!("sbol_db_jobs_listener_connected").set(0.0);
                        return;
                    }
                    msg = stream.next() => match msg {
                        Some(Ok(notification)) => {
                            // Filter by queue: NOTIFY payload is the queue name.
                            let payload = notification.payload();
                            if queues.iter().any(|q| q == payload) {
                                metrics::counter!(
                                    "sbol_db_jobs_notifications_received_total",
                                    "queue" => payload.to_owned(),
                                )
                                .increment(1);
                                let _ = notify.send(());
                            }
                        }
                        Some(Err(err)) => {
                            tracing::warn!(error = %err, "listener stream error; reconnecting");
                            metrics::counter!(
                                "sbol_db_jobs_listener_reconnects_total",
                                "reason" => "stream_error",
                            )
                            .increment(1);
                            break;
                        }
                        None => {
                            tracing::warn!("listener stream closed; reconnecting");
                            metrics::counter!(
                                "sbol_db_jobs_listener_reconnects_total",
                                "reason" => "stream_closed",
                            )
                            .increment(1);
                            break;
                        }
                    }
                }
            }
            metrics::gauge!("sbol_db_jobs_listener_connected").set(0.0);
        }
    })
}
