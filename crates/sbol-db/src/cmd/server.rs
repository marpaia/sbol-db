//! `sbol-db server` — start the HTTP listener and, by default, an embedded
//! async-job worker. The worker uses its own connection pool so long-running
//! handlers can't starve inbound HTTP requests.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use sbol_db_jobs::{default_registry, Worker, WorkerConfig};
use sbol_db_postgres::{JobRepository, SbolObjectService};
use sbol_db_server::{router, AppState, Metrics};
use sbol_db_sparql::SparqlEngine;
use tokio_util::sync::CancellationToken;

use crate::signal::shutdown_signal;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    pool: sbol_db_postgres::PgPool,
    service: Arc<SbolObjectService>,
    database_url: &str,
    bind: SocketAddr,
    no_worker: bool,
    worker_concurrency: Option<usize>,
    worker_queues: Option<String>,
    worker_id: Option<String>,
) -> Result<()> {
    let engine = Arc::new(SparqlEngine::new(Arc::new(service.triples().clone())));
    let jobs_repo = Arc::new(JobRepository::new(pool.clone()));

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

    let metrics = Metrics::install(pool.clone(), env!("CARGO_PKG_VERSION"));
    let metrics = metrics.with_jobs_repo(jobs_repo.clone());
    let metrics = if let Some(setup) = worker_setup.as_ref() {
        metrics.with_worker_pool(setup.pool.clone())
    } else {
        metrics
    };

    let config = sbol_db_server::ServerConfig::from_env();
    let state = AppState {
        service: service.clone(),
        sparql: engine,
        metrics,
        jobs: jobs_repo,
        config: config.clone(),
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
    pub service: Arc<SbolObjectService>,
    pub config: WorkerConfig,
}

impl WorkerSetup {
    pub fn spawn(self, cancel: CancellationToken) -> tokio::task::JoinHandle<()> {
        let registry = Arc::new(default_registry());
        let worker = Worker::new(self.pool, self.service, registry, self.config);
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
        None => vec![sbol_db_postgres::DEFAULT_QUEUE.to_owned()],
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
    let service = Arc::new(SbolObjectService::new(pool.clone()));

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
        service,
        config,
    })
}
