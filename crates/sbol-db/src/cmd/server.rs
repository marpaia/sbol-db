//! `sbol-db server` — start the HTTP listener and, by default, an embedded
//! async-job worker. On Postgres the worker opens its own right-sized
//! connection pool so long-running handlers can't starve inbound HTTP; other
//! backends open through the factory.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use sbol_db_backend::Backend;
use sbol_db_jobs::{default_registry, Worker, WorkerConfig};
use sbol_db_server::{router, AppState, Metrics};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use sbol_db_storage::{JobQueue, SbolStore};
use tokio_util::sync::CancellationToken;

use crate::signal::shutdown_signal;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    backend: Backend,
    database_url: &str,
    bind: SocketAddr,
    no_worker: bool,
    worker_concurrency: Option<usize>,
    worker_queues: Option<String>,
    worker_id: Option<String>,
) -> Result<()> {
    let engine = Arc::new(match backend.native_sparql.clone() {
        Some(native) => SparqlEngine::with_native(backend.triple_source.clone(), native),
        None => SparqlEngine::new(backend.triple_source.clone()),
    });

    let worker_setup = if !no_worker {
        Some(
            build_worker_setup(
                database_url,
                Some((backend.store.clone(), backend.jobs.clone())),
                worker_concurrency,
                worker_queues.as_deref(),
                worker_id.as_deref(),
            )
            .await?,
        )
    } else {
        tracing::info!("embedded worker disabled (--no-worker); HTTP-only node");
        None
    };

    // The API pool, when the backend exposes one, drives the connection-pool
    // gauges; poolless backends simply omit them.
    let api_pool = backend.postgres.as_ref().map(|pg| pg.pool.clone());
    let metrics = Metrics::install(api_pool, env!("CARGO_PKG_VERSION"));
    let metrics = metrics.with_jobs_repo(backend.jobs.clone());
    let metrics = match worker_setup.as_ref().and_then(|s| s.listener_pool.as_ref()) {
        Some(pool) => metrics.with_worker_pool(pool.clone()),
        None => metrics,
    };

    let config = sbol_db_server::ServerConfig::from_env();
    let sparql_update = Arc::new(SparqlUpdateEngine::new(
        backend.triple_source.clone(),
        backend.triple_writer.clone(),
    ));
    let state = AppState {
        service: backend.store.clone(),
        sparql: engine,
        sparql_update,
        metrics,
        jobs: backend.jobs.clone(),
        config: config.clone(),
        // Backend-neutral lab dashboard / graph browser.
        #[cfg(feature = "lab")]
        lab: backend.lab.clone(),
        // The lab's SQL console and introspection are Postgres-only; they
        // degrade to a clear error on other backends. Present only for Postgres.
        #[cfg(feature = "lab")]
        pg_pool: backend.postgres.as_ref().map(|pg| pg.pool.clone()),
        #[cfg(feature = "lab")]
        schema_cache: Arc::new(sbol_db_server::SchemaCache::new()),
    };
    let app = router(state, config);

    let cancel = CancellationToken::new();
    let worker_handle = worker_setup.map(|setup| setup.spawn(cancel.clone()));

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, worker = worker_handle.is_some(), "sbol-db serving");
    println!("sbol-db listening on http://{bind}");

    let cancel_for_shutdown = cancel.clone();
    let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
        shutdown_signal().await;
        cancel_for_shutdown.cancel();
    });
    serve.await?;
    tracing::info!("HTTP listener stopped; waiting for embedded worker to drain");

    if let Some(handle) = worker_handle {
        if let Err(err) = handle.await {
            tracing::warn!(error = %err, "embedded worker task panicked");
        }
    }
    tracing::info!("sbol-db server loop exited cleanly");
    Ok(())
}

/// Constructed setup for an embedded / standalone worker: the backend-neutral
/// store + job queue, an optional Postgres pool (the LISTEN/NOTIFY channel and
/// worker-pool gauge source), and the worker config. Split from spawning so
/// callers can hand the pool to `Metrics::with_worker_pool` before the worker
/// starts taking work.
pub(crate) struct WorkerSetup {
    pub listener_pool: Option<sbol_db_postgres::PgPool>,
    pub store: Arc<dyn SbolStore>,
    pub jobs: Arc<dyn JobQueue>,
    pub config: WorkerConfig,
}

impl WorkerSetup {
    pub fn spawn(self, cancel: CancellationToken) -> tokio::task::JoinHandle<()> {
        let registry = Arc::new(default_registry());
        // `listener_pool` (Postgres only) doubles as the LISTEN/NOTIFY channel
        // for low-latency wakeups; without it the worker falls back to polling.
        let worker = Worker::new(
            self.jobs,
            self.store,
            self.listener_pool,
            registry,
            self.config,
        );
        tokio::spawn(async move {
            if let Err(err) = worker.run(cancel).await {
                tracing::error!(error = %err, "embedded worker exited with error");
            }
        })
    }
}

/// Build the worker's store, job queue, and config. On Postgres the worker opens
/// its own right-sized pool so long-running handlers cannot starve inbound HTTP
/// requests. Other backends reuse the already-open API handle: SQLite avoids a
/// redundant second pool, and RocksDB must (its database lock is exclusive to a
/// single open handle per process).
pub(crate) async fn build_worker_setup(
    database_url: &str,
    reuse: Option<(Arc<dyn SbolStore>, Arc<dyn JobQueue>)>,
    concurrency: Option<usize>,
    queues: Option<&str>,
    worker_id: Option<&str>,
) -> Result<WorkerSetup> {
    let concurrency = concurrency.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    });
    let queue_list: Vec<String> = match queues {
        None => vec![sbol_db_storage::DEFAULT_QUEUE.to_owned()],
        Some(s) => s
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
    };

    // Postgres opens a dedicated worker pool; every other backend reuses the
    // store and queue the API already opened.
    let (listener_pool, store, jobs) = if is_postgres(database_url) {
        let mut worker_pool_cfg = sbol_db_postgres::PoolConfig::from_env();
        let override_max = std::env::var("SBOL_DB_WORKER_POOL_MAX")
            .ok()
            .and_then(|s| s.parse::<u32>().ok());
        worker_pool_cfg.max_connections = override_max.unwrap_or((concurrency as u32) + 4);
        let pool = sbol_db_postgres::pool::connect_with_config(database_url, &worker_pool_cfg)
            .await
            .context("opening worker connection pool")?;
        let backend = Backend::from_postgres_pool(pool.clone());
        (Some(pool), backend.store, backend.jobs)
    } else {
        // Reuse the API's already-open handle when there is one (required for
        // RocksDB, whose lock is exclusive); otherwise open it ourselves.
        let (store, jobs) = match reuse {
            Some(pair) => pair,
            None => {
                let backend = Backend::open(database_url).await?;
                (backend.store, backend.jobs)
            }
        };
        (None, store, jobs)
    };

    let mut config = WorkerConfig {
        concurrency,
        queues: queue_list,
        ..WorkerConfig::default()
    };
    if let Some(id) = worker_id {
        config.worker_id = id.into();
    }

    Ok(WorkerSetup {
        listener_pool,
        store,
        jobs,
        config,
    })
}

fn is_postgres(url: &str) -> bool {
    url.starts_with("postgres://") || url.starts_with("postgresql://")
}
