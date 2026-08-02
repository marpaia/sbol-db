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

mod backup_scheduler;
mod cli;
mod cmd;
mod edge_config;
mod format;
mod output;
mod runtime;
mod search_config;
mod signal;
mod tls;

use crate::cli::{Cli, Command};
use crate::runtime::{resolve_connection, ServerRuntime};

#[tokio::main]
async fn main() -> Result<()> {
    init_logging();
    let cli = Cli::parse();

    // `util` runs with no database, so it works in environments where
    // Postgres isn't reachable.
    if let Command::Util { action } = cli.command {
        return cmd::util::run(action).await;
    }
    if let Command::Backup { action } = cli.command {
        return cmd::backup::run(action);
    }

    // RDF acquisition and normalization are target-free and preserve the raw
    // export, so they must not open or mutate a destination database.
    if let Command::NormalizeSynbiohubRdf {
        input,
        output,
        policy,
        report,
    } = cli.command
    {
        return cmd::migrate::normalize::run(cmd::migrate::normalize::NormalizeInputs {
            input,
            output,
            policy,
            report,
        });
    }

    // Source preflight is intentionally target-free: it must work before a
    // destination database exists and must never mutate one accidentally.
    if let Command::PreflightSynbiohub {
        source,
        virtuoso_db,
        rdf,
        rdf_normalization_report,
        sqlite,
        uploads,
        config,
        config_defaults,
        report,
        allow_blockers,
    } = cli.command
    {
        return cmd::migrate::preflight::run(cmd::migrate::preflight::PreflightInputs {
            source,
            virtuoso_db,
            rdf,
            rdf_normalization_report,
            sqlite,
            uploads,
            config,
            config_defaults,
            report,
            allow_blockers,
        })
        .await;
    }

    if let Command::CopyPostgresToRocksdb {
        destination,
        chunk_size,
        omit_completed_job_history,
    } = cli.command
    {
        let source_url = resolve_connection(cli.backend, cli.database_url.as_deref())?;
        return cmd::migrate::rocksdb::run(cmd::migrate::rocksdb::CopyInputs {
            source_url,
            destination,
            chunk_size,
            omit_completed_job_history,
        })
        .await;
    }

    let mut server_runtime = match &cli.command {
        Command::Server { args } => Some(ServerRuntime::resolve(
            args.profile,
            args.data_dir.as_deref(),
            args.blob_root.as_deref(),
            cli.backend,
            cli.database_url.as_deref(),
        )?),
        _ => None,
    };
    let database_url = match server_runtime.as_ref() {
        Some(runtime) => runtime.database_url().to_owned(),
        None => resolve_connection(cli.backend, cli.database_url.as_deref())?,
    };
    let backend = open_backend(&database_url, &cli.command).await?;

    match cli.command {
        Command::Server { args } => {
            cmd::server::run(
                backend,
                server_runtime
                    .take()
                    .expect("server runtime is resolved before backend open"),
                *args,
            )
            .await
        }
        Command::Worker {
            concurrency,
            queues,
            worker_id,
            search_config,
        } => cmd::worker::run(&database_url, concurrency, queues, worker_id, search_config).await,
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
        Command::MigrateSynbiohub {
            manifest,
            policy,
            blob_store,
            chunk_size,
            no_reindex,
        } => {
            let pool = backend.require_postgres()?.pool.clone();
            cmd::migrate::production::run(
                pool,
                backend.config.clone(),
                backend.jobs.clone(),
                cmd::migrate::production::ProductionInputs {
                    manifest,
                    policy,
                    blob_store,
                    chunk_size,
                    no_reindex,
                },
            )
            .await
        }
        Command::PreflightSynbiohub { .. } => unreachable!("handled before backend open"),
        Command::NormalizeSynbiohubRdf { .. } => unreachable!("handled before backend open"),
        Command::CopyPostgresToRocksdb { .. } => unreachable!("handled before backend open"),
        Command::Util { .. } => unreachable!("handled before backend open"),
        Command::Backup { .. } => unreachable!("handled before backend open"),
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
#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use clap::CommandFactory;

    use super::*;
    use crate::cli::BackendKind;

    #[test]
    fn database_argument_defers_its_default_to_runtime_resolution() {
        // Inspect the clap schema rather than parsing the ambient process
        // environment: CI intentionally sets DATABASE_URL for Postgres tests,
        // and clap's env source correctly takes precedence over runtime fallback.
        let command = Cli::command();
        let database_url = command
            .get_arguments()
            .find(|argument| argument.get_id() == "database_url")
            .expect("database_url argument");
        assert!(database_url.get_default_values().is_empty());
        assert_eq!(database_url.get_env(), Some(OsStr::new("DATABASE_URL")));

        let cli = Cli::try_parse_from([
            "sbol-db",
            "--database-url",
            crate::cli::DEFAULT_LOCAL_DATABASE_URL,
            "server",
        ])
        .unwrap();
        assert_eq!(cli.backend, None);
        assert_eq!(
            cli.database_url.as_deref(),
            Some(crate::cli::DEFAULT_LOCAL_DATABASE_URL)
        );
        assert!(matches!(cli.command, Command::Server { .. }));
        assert_eq!(
            resolve_connection(None, None).unwrap(),
            crate::cli::DEFAULT_LOCAL_DATABASE_URL
        );
    }

    #[test]
    fn no_selector_passes_url_through() {
        assert_eq!(
            resolve_connection(None, Some("sqlite:///tmp/x.db")).unwrap(),
            "sqlite:///tmp/x.db"
        );
    }

    #[test]
    fn selector_accepts_matching_scheme() {
        assert_eq!(
            resolve_connection(Some(BackendKind::Rocksdb), Some("rocksdb:///data/x")).unwrap(),
            "rocksdb:///data/x"
        );
        // Postgres answers to both schemes.
        assert_eq!(
            resolve_connection(Some(BackendKind::Postgres), Some("postgresql://h/db")).unwrap(),
            "postgresql://h/db"
        );
    }

    #[test]
    fn selector_completes_a_bare_path() {
        assert_eq!(
            resolve_connection(Some(BackendKind::Rocksdb), Some("/var/lib/sbol.rocksdb")).unwrap(),
            "rocksdb:///var/lib/sbol.rocksdb"
        );
        assert_eq!(
            resolve_connection(Some(BackendKind::Sqlite), Some("/tmp/x.db")).unwrap(),
            "sqlite:///tmp/x.db"
        );
    }

    #[test]
    fn selector_rejects_conflicting_scheme() {
        let err = resolve_connection(
            Some(BackendKind::Sqlite),
            Some("postgres://sbol:sbol@localhost/sbol"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("conflicts"), "got: {err}");
    }
}
