//! `sbol-db` CLI entry point. Noun-first: top-level commands are nouns
//! (`doc`, `object`, `query`, ...), each with its own verbs. Daemons
//! (`server`, `worker`) stay top-level because they are the noun.
//!
//! `main` parses the CLI, opens the storage backend when the command needs
//! it, and dispatches to a per-noun handler under `cmd::*`. Local utilities
//! (`util`) skip the backend open entirely.

use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use sbol_db_backend::Backend;

mod cli;
mod cmd;
mod format;
mod output;
mod signal;

use crate::cli::{Cli, Command};

#[tokio::main]
async fn main() -> Result<()> {
    init_logging();
    let cli = Cli::parse();

    // `util` runs with no database, so it works in environments where
    // Postgres isn't reachable.
    if let Command::Util { action } = cli.command {
        return cmd::util::run(action).await;
    }

    let backend = open_backend(&cli.database_url, &cli.command).await?;

    match cli.command {
        Command::Server {
            bind,
            no_worker,
            worker_concurrency,
            worker_queues,
            worker_id,
        } => {
            cmd::server::run(
                backend,
                &cli.database_url,
                bind,
                no_worker,
                worker_concurrency,
                worker_queues,
                worker_id,
            )
            .await
        }
        Command::Worker {
            concurrency,
            queues,
            worker_id,
        } => cmd::worker::run(&cli.database_url, concurrency, queues, worker_id).await,
        Command::Graph { action } => cmd::graph::run(backend.store.clone(), action).await,
        Command::Object { action } => cmd::object::run(backend.store.clone(), action).await,
        Command::Query { action } => {
            cmd::query::run(backend.store.clone(), backend.triple_source.clone(), action).await
        }
        Command::Ontology { action } => cmd::ontology::run(backend.store.clone(), action).await,
        Command::Jobs { action } => cmd::jobs::run(backend.jobs.clone(), action).await,
        Command::Db { action } => {
            let migrator = backend
                .migrator
                .clone()
                .context("the db command requires a backend with migration support")?;
            cmd::db::run(
                migrator,
                backend.store.clone(),
                backend.jobs.clone(),
                action,
            )
            .await
        }
        Command::Inspect { action } => {
            let stats = backend
                .db_stats
                .clone()
                .context("the inspect command requires a backend with introspection support")?;
            cmd::inspect::run(stats, action).await
        }
        Command::Util { .. } => unreachable!("handled before backend open"),
    }
}

fn init_logging() {
    use std::io::IsTerminal;
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let want_json = match std::env::var("LOG_FORMAT")
        .ok()
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("json") => true,
        Some("text") | Some("plain") | Some("human") => false,
        _ => !std::io::stdout().is_terminal(),
    };
    if want_json {
        let _ = fmt()
            .with_env_filter(filter)
            .with_target(false)
            .json()
            .try_init();
    } else {
        let _ = fmt().with_env_filter(filter).with_target(false).try_init();
    }
}

/// Open the storage backend selected by `database_url`'s scheme. Commands that
/// need a long startup retry loop honor `DATABASE_STARTUP_TIMEOUT_SECS`;
/// everything else fails fast on the first connection error.
async fn open_backend(database_url: &str, command: &Command) -> Result<Backend> {
    let needs_retry = matches!(
        command,
        Command::Server { .. } | Command::Worker { .. } | Command::Db { .. }
    );
    let deadline = if needs_retry {
        Duration::from_secs(
            std::env::var("DATABASE_STARTUP_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
        )
    } else {
        Duration::ZERO
    };
    Backend::open_with_retry(database_url, deadline).await
}
