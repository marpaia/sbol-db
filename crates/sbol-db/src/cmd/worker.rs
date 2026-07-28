//! `sbol-db worker` — standalone async-job worker, no HTTP listener.
//! Stops on SIGTERM / Ctrl-C; in-flight handlers get a grace window
//! before their leases are abandoned.

use std::path::PathBuf;

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use crate::cmd::server::build_worker_setup;
use crate::signal::shutdown_signal;

pub async fn run(
    database_url: &str,
    concurrency: Option<usize>,
    queues: Option<String>,
    worker_id: Option<String>,
    search_config: Option<PathBuf>,
) -> Result<()> {
    let cancel = CancellationToken::new();
    let mut setup = build_worker_setup(
        database_url,
        None,
        concurrency,
        queues.as_deref(),
        worker_id.as_deref(),
    )
    .await?;
    if let Some(path) = search_config {
        setup.vector_indexes = Some(
            crate::search_config::load_builder(&path)
                .await?
                .build_maintenance()?,
        );
        tracing::info!(path = %path.display(), "vector maintenance plugins configured");
    }
    let handle = setup.spawn(cancel.clone());
    tracing::info!("standalone worker started");
    shutdown_signal().await;
    cancel.cancel();
    if let Err(err) = handle.await {
        tracing::warn!(error = %err, "worker task panicked");
    }
    Ok(())
}
