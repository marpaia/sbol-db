//! `sbol-db server` — start the HTTP listener and, by default, an embedded
//! async-job worker. The worker uses its own connection pool so long-running
//! handlers can't starve inbound HTTP requests.

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
    let engine = Arc::new(SparqlEngine::new(backend.triple_source.clone()));

    let worker_setup = if !no_worker {
        Some(
            build_worker_setup(
                database_url,
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
    let metrics = if let Some(setup) = worker_setup.as_ref() {
        metrics.with_worker_pool(setup.pool.clone())
    } else {
        metrics
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
        // The lab's SQL console and introspection are irreducibly Postgres;
        // mounting the lab requires the Postgres backend.
        #[cfg(feature = "lab")]
        pg_pool: backend
            .require_postgres()
            .context("the lab feature requires the Postgres backend")?
            .pool
            .clone(),
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

/// Constructed setup for an embedded / standalone worker: the separate
/// connection pool, the service that wraps it, and the worker config.
/// Split from spawning so callers (e.g. `serve`) can hand the pool to
/// `Metrics::with_worker_pool` before the worker starts taking work.
pub(crate) struct WorkerSetup {
    pub pool: sbol_db_postgres::PgPool,
    pub store: Arc<dyn SbolStore>,
    pub jobs: Arc<dyn JobQueue>,
    pub config: WorkerConfig,
}

impl WorkerSetup {
    pub fn spawn(self, cancel: CancellationToken) -> tokio::task::JoinHandle<()> {
        let registry = Arc::new(default_registry());
        // The pool doubles as the LISTEN/NOTIFY channel for low-latency wakeups.
        let worker = Worker::new(
            self.jobs,
            self.store,
            Some(self.pool),
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

/// Build the worker pool, service, and config. The worker shares Postgres
/// with the API but keeps its own connection pool so long-running handlers
/// cannot starve inbound HTTP requests.
pub(crate) async fn build_worker_setup(
    database_url: &str,
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

    let mut worker_pool_cfg = sbol_db_postgres::PoolConfig::from_env();
    let override_max = std::env::var("SBOL_DB_WORKER_POOL_MAX")
        .ok()
        .and_then(|s| s.parse::<u32>().ok());
    worker_pool_cfg.max_connections = override_max.unwrap_or((concurrency as u32) + 4);

    let pool = sbol_db_postgres::pool::connect_with_config(database_url, &worker_pool_cfg)
        .await
        .context("opening worker connection pool")?;
    let bundle = Backend::from_postgres_pool(pool.clone());

    let mut config = WorkerConfig {
        concurrency,
        queues: queue_list,
        ..WorkerConfig::default()
    };
    if let Some(id) = worker_id {
        config.worker_id = id.into();
    }

    Ok(WorkerSetup {
        pool,
        store: bundle.store,
        jobs: bundle.jobs,
        config,
    })
}
