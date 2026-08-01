//! `sbol-db server` — start the HTTP listener and, by default, an embedded
//! async-job worker. On Postgres the worker opens its own right-sized
//! connection pool so long-running handlers can't starve inbound HTTP; other
//! backends open through the factory.

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use sbol_db_app::{AppServices, FsBlobStore, LegacyExplorerStrategy};
use sbol_db_backend::Backend;
use sbol_db_jobs::{default_registry, SearchIndexHandles, Worker, WorkerConfig};
use sbol_db_search::VectorIndexMaintainerRegistry;
use sbol_db_search_sdk::IndexMutationSource;
use sbol_db_server::{
    explorer_router, operations_router, public_router, AppState, Metrics, ServerProfile,
};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use sbol_db_storage::{ConfigStore, JobQueue, SbolStore};
use tokio_util::sync::CancellationToken;

use crate::cli::ServerArgs;
use crate::runtime::ServerRuntime;
use crate::signal::shutdown_signal;

pub async fn run(backend: Backend, runtime: ServerRuntime, args: ServerArgs) -> Result<()> {
    let ServerArgs {
        profile: _,
        data_dir: _,
        blob_root: _,
        bind,
        operations_bind,
        explorer_bind,
        no_worker,
        worker_concurrency,
        worker_queues,
        worker_id,
        search_config,
    } = args;
    let production = runtime.profile() == crate::cli::RuntimeProfile::Production;
    if production && !operations_bind.ip().is_loopback() {
        bail!(
            "production --operations-bind must use a loopback address; \
             run the local telemetry collector on the same server"
        );
    }
    let database_url = runtime.database_url().to_owned();
    let engine = Arc::new(SparqlEngine::new(backend.triple_source.clone()));

    let mut worker_setup = if !no_worker {
        Some(
            build_worker_setup(
                &database_url,
                Some((
                    backend.store.clone(),
                    backend.jobs.clone(),
                    backend.config.clone(),
                )),
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

    let config = sbol_db_server::ServerConfig::from_env_for(if production {
        ServerProfile::Production
    } else {
        ServerProfile::Development
    })?;
    let sparql_update = Arc::new(SparqlUpdateEngine::new(
        backend.triple_source.clone(),
        backend.triple_writer.clone(),
    ));
    let mut app_services = AppServices::from_backend(&backend)
        .with_blobs(Arc::new(FsBlobStore::new(runtime.blob_root())));
    if let Some(layout) = runtime.layout() {
        let text_index = Arc::new(
            sbol_db_search::ranked_text::RankedTextIndex::open_or_create(layout.search_root())
                .with_context(|| {
                    format!(
                        "opening ranked text index at {}",
                        layout.search_root().display()
                    )
                })?,
        );
        app_services = app_services.with_text_search(text_index);
        tracing::info!(
            profile = ?runtime.profile(),
            data_dir = %layout.root().display(),
            generation = %layout.generation(),
            generation_root = %layout.generation_root().display(),
            blob_root = %runtime.blob_root().display(),
            acme_root = %layout.acme_root().display(),
            backups_root = %layout.backups_root().display(),
            restore_root = %layout.restore_root().display(),
            "managed production data layout active"
        );
    } else {
        tracing::info!(
            profile = ?runtime.profile(),
            blob_root = %runtime.blob_root().display(),
            "server runtime configured"
        );
    }
    if production && !app_services.auth.any_admin().await? && config.setup_token_hash.is_none() {
        bail!(
            "a fresh production instance requires SBOL_DB_SETUP_TOKEN with at least 32 characters; \
             provide it for first-launch setup, then remove it after creating the administrator"
        );
    }

    let built_in_search = search_config.is_none();
    if built_in_search && no_worker {
        tracing::warn!(
            "the built-in in-process vector index is disabled with --no-worker; \
             configure a shared vector backend with --search-config for an external worker"
        );
    } else {
        let deployment = match search_config {
            Some(path) => crate::search_config::load_builder(&path)
                .await?
                .register_strategy(Arc::new(LegacyExplorerStrategy::new(
                    app_services.text_search.clone(),
                    app_services.cluster.clone(),
                )))?
                .build()?,
            None => crate::search_config::built_in_text_deployment().await?,
        };
        if let Some(setup) = worker_setup.as_mut() {
            setup.vector_indexes = Some(deployment.maintainers());
        }
        app_services = app_services.with_search_deployment(&deployment);
        if built_in_search {
            app_services
                .schedule_search_reconciliation(IndexMutationSource::Startup)
                .await?;
        }
        tracing::info!(
            strategies = app_services.search_runtime().descriptors().len(),
            vector_indexes = deployment.maintainers().len(),
            built_in = built_in_search,
            "search plugin deployment configured"
        );
    }
    let app_services = Arc::new(app_services);

    // The embedded worker shares this process, so it reindexes into the very
    // same ranked text index the API reads, alongside the backend's durable
    // cluster and PageRank stores and its triple source. This is what lets the
    // `rebuild_search_index` job populate the clusters and PageRank that back
    // `/similar` and the ranked search.
    if let Some(setup) = worker_setup.as_mut() {
        setup.search = Some(SearchIndexHandles {
            cluster: backend.cluster.clone(),
            pagerank: backend.pagerank.clone(),
            sketch: backend.sketch.clone(),
            text_index: app_services.text_search.clone(),
            triples: backend.triple_source.clone(),
        });
    }

    let state = AppState {
        service: backend.store.clone(),
        sparql: engine,
        sparql_update,
        app: app_services,
        metrics,
        jobs: backend.jobs.clone(),
        config: config.clone(),
        // Backend-neutral lab dashboard / graph browser.
        #[cfg(feature = "lab")]
        lab: backend.lab.clone(),
        // Capability handles for the lab's engine-specific pages. Each is
        // present only on a backend that supports it; the pages degrade to a
        // clear error otherwise and the UI gates them on `/lab/api/info`.
        #[cfg(feature = "lab")]
        backend_kind: backend.kind,
        #[cfg(feature = "lab")]
        sql_console: backend.sql_console.clone(),
        #[cfg(feature = "lab")]
        db_stats: backend.db_stats.clone(),
        #[cfg(feature = "lab")]
        lsm_stats: backend.lsm_stats.clone(),
        #[cfg(feature = "lab")]
        schema_cache: Arc::new(sbol_db_server::SchemaCache::new()),
    };
    let app = public_router(state.clone(), config.clone());
    let operations_app = operations_router(state.clone(), config.clone());
    let explorer_app = explorer_bind.map(|_| explorer_router(state, config.clone()));

    let cancel = CancellationToken::new();
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("bind public listener at {bind}"))?;
    let bind = listener
        .local_addr()
        .context("read public listener address")?;
    let operations_listener = tokio::net::TcpListener::bind(operations_bind)
        .await
        .with_context(|| format!("bind operations listener at {operations_bind}"))?;
    let operations_bind = operations_listener
        .local_addr()
        .context("read operations listener address")?;
    let explorer_listener = match explorer_bind {
        Some(explorer_bind) => {
            let listener = tokio::net::TcpListener::bind(explorer_bind)
                .await
                .with_context(|| format!("bind SBOLExplorer listener at {explorer_bind}"))?;
            let explorer_bind = listener
                .local_addr()
                .context("read SBOLExplorer listener address")?;
            Some((explorer_bind, listener))
        }
        None => None,
    };
    let worker_handle = worker_setup.map(|setup| setup.spawn(cancel.clone()));
    tracing::info!(%bind, %operations_bind, worker = worker_handle.is_some(), "sbol-db serving");
    println!("sbol-db listening on http://{bind}");
    println!("sbol-db operations listening on http://{operations_bind}");
    if let Some((explorer_bind, _)) = explorer_listener.as_ref() {
        tracing::info!(%explorer_bind, "SBOLExplorer compatibility listener serving");
        println!("sbol-db Explorer compatibility listening on http://{explorer_bind}");
    }

    let main_cancel = cancel.clone();
    let mut main_server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(main_cancel.cancelled_owned())
            .await
    });
    let operations_cancel = cancel.clone();
    let mut operations_server = tokio::spawn(async move {
        axum::serve(operations_listener, operations_app)
            .with_graceful_shutdown(operations_cancel.cancelled_owned())
            .await
    });
    let mut explorer_server = match (explorer_listener, explorer_app) {
        (Some((_bind, listener)), Some(app)) => {
            let explorer_cancel = cancel.clone();
            Some(tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(explorer_cancel.cancelled_owned())
                    .await
            }))
        }
        _ => None,
    };

    enum FirstExit {
        Signal,
        Main(std::result::Result<std::io::Result<()>, tokio::task::JoinError>),
        Operations(std::result::Result<std::io::Result<()>, tokio::task::JoinError>),
        Explorer(std::result::Result<std::io::Result<()>, tokio::task::JoinError>),
    }

    let first_exit = if let Some(explorer) = explorer_server.as_mut() {
        tokio::select! {
            _ = shutdown_signal() => FirstExit::Signal,
            result = &mut main_server => FirstExit::Main(result),
            result = &mut operations_server => FirstExit::Operations(result),
            result = explorer => FirstExit::Explorer(result),
        }
    } else {
        tokio::select! {
            _ = shutdown_signal() => FirstExit::Signal,
            result = &mut main_server => FirstExit::Main(result),
            result = &mut operations_server => FirstExit::Operations(result),
        }
    };
    cancel.cancel();

    let mut main_completed = false;
    let mut operations_completed = false;
    let mut explorer_completed = false;
    match first_exit {
        FirstExit::Signal => {}
        FirstExit::Main(result) => {
            main_completed = true;
            result.context("main HTTP listener task")??;
        }
        FirstExit::Operations(result) => {
            operations_completed = true;
            result.context("operations HTTP listener task")??;
        }
        FirstExit::Explorer(result) => {
            explorer_completed = true;
            result.context("SBOLExplorer listener task")??;
        }
    }
    if !main_completed {
        main_server.await.context("main HTTP listener task")??;
    }
    if !operations_completed {
        operations_server
            .await
            .context("operations HTTP listener task")??;
    }
    if !explorer_completed {
        if let Some(explorer) = explorer_server {
            explorer.await.context("SBOLExplorer listener task")??;
        }
    }
    tracing::info!("HTTP listeners stopped; waiting for embedded worker to drain");

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
    pub config_store: Arc<dyn ConfigStore>,
    pub config: WorkerConfig,
    /// The shared search-index handles the `rebuild_search_index` job needs.
    /// Present only for the embedded worker, which shares the API's process and
    /// therefore its in-RAM ranked text index; the standalone `worker`
    /// subcommand runs in a separate process with no shared index and leaves it
    /// `None`, so that job kind fails fast there.
    pub search: Option<SearchIndexHandles>,
    /// Maintenance coordinators keyed by logical vector index. A deployment
    /// that assembles search plugins installs the same validated registry used
    /// by its query router.
    pub vector_indexes: Option<Arc<VectorIndexMaintainerRegistry>>,
}

impl WorkerSetup {
    pub fn spawn(self, cancel: CancellationToken) -> tokio::task::JoinHandle<()> {
        let registry = Arc::new(default_registry());
        // `listener_pool` (Postgres only) doubles as the LISTEN/NOTIFY channel
        // for low-latency wakeups; without it the worker falls back to polling.
        // The config store lets the `wor_sync` job read the joined Web of
        // Registries URL and persist the pulled prefix map.
        let mut worker = Worker::new(
            self.jobs,
            self.store,
            self.listener_pool,
            registry,
            self.config,
        )
        .with_config_store(self.config_store);
        if let Some(search) = self.search {
            worker = worker.with_search_index(search);
        }
        if let Some(vector_indexes) = self.vector_indexes {
            worker = worker.with_vector_indexes(vector_indexes);
        }
        tokio::spawn(async move {
            if let Err(err) = worker.run(cancel).await {
                tracing::error!(error = %err, "embedded worker exited with error");
            }
        })
    }
}

/// The already-open handles the API hands the worker to reuse: the SBOL store,
/// the job queue, and the durable config store.
type ReusableHandles = (Arc<dyn SbolStore>, Arc<dyn JobQueue>, Arc<dyn ConfigStore>);

/// Build the worker's store, job queue, and config. On Postgres the worker opens
/// its own right-sized pool so long-running handlers cannot starve inbound HTTP
/// requests. Other backends reuse the already-open API handle: SQLite avoids a
/// redundant second pool, and RocksDB must (its database lock is exclusive to a
/// single open handle per process).
pub(crate) async fn build_worker_setup(
    database_url: &str,
    reuse: Option<ReusableHandles>,
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
    let (listener_pool, store, jobs, config_store) = if is_postgres(database_url) {
        let mut worker_pool_cfg = sbol_db_postgres::PoolConfig::from_env();
        let override_max = std::env::var("SBOL_DB_WORKER_POOL_MAX")
            .ok()
            .and_then(|s| s.parse::<u32>().ok());
        worker_pool_cfg.max_connections = override_max.unwrap_or((concurrency as u32) + 4);
        let pool = sbol_db_postgres::pool::connect_with_config(database_url, &worker_pool_cfg)
            .await
            .context("opening worker connection pool")?;
        let backend = Backend::from_postgres_pool(pool.clone());
        (Some(pool), backend.store, backend.jobs, backend.config)
    } else {
        // Reuse the API's already-open handle when there is one (required for
        // RocksDB, whose lock is exclusive); otherwise open it ourselves.
        let (store, jobs, config_store) = match reuse {
            Some(handles) => handles,
            None => {
                let backend = Backend::open(database_url).await?;
                (backend.store, backend.jobs, backend.config)
            }
        };
        (None, store, jobs, config_store)
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
        config_store,
        config,
        // Wired by the embedded-server path once the shared search index exists;
        // the standalone worker leaves it unset.
        search: None,
        // Installed by a configured search deployment.
        vector_indexes: None,
    })
}

fn is_postgres(url: &str) -> bool {
    url.starts_with("postgres://") || url.starts_with("postgresql://")
}
