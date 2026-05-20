//! `sbol-db inspect` — read-only Postgres inspection. Each subcommand
//! wraps one method on `PgStatsRepository`, or prints the effective
//! `ServerConfig`. Output is always JSON.

use anyhow::Result;
use sbol_db_postgres::{PgPool, PgStatsRepository};
use sbol_db_server::ServerConfig;

use crate::cli::InspectAction;
use crate::output::print_json;

pub async fn run(pool: PgPool, action: InspectAction) -> Result<()> {
    let repo = PgStatsRepository::new(pool);
    match action {
        InspectAction::Size => {
            let size = repo.database_size().await?;
            print_json(&size)
        }
        InspectAction::Tables { limit, offset } => {
            let rows = repo.table_stats(limit, offset).await?;
            print_json(&rows)
        }
        InspectAction::Table { name } => {
            let schema = repo.table_schema(&name).await?;
            match schema {
                Some(s) => print_json(&s),
                None => Err(anyhow::anyhow!(
                    "no table named {name} in the public schema"
                )),
            }
        }
        InspectAction::Activity {
            limit,
            include_idle,
        } => {
            let rows = repo.activity(limit).await?;
            let filtered: Vec<_> = if include_idle {
                rows
            } else {
                rows.into_iter()
                    .filter(|a| a.state.as_deref() != Some("idle"))
                    .collect()
            };
            print_json(&filtered)
        }
        InspectAction::Locks => {
            let rows = repo.blocking_locks().await?;
            print_json(&rows)
        }
        InspectAction::Indexes { limit } => {
            let rows = repo.index_stats(limit).await?;
            print_json(&rows)
        }
        InspectAction::SlowQueries { limit } => {
            if !repo.has_pg_stat_statements().await? {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "reason": "pg_stat_statements extension is not installed; \
                                   enable it in postgresql.conf and CREATE EXTENSION",
                    })
                );
                return Ok(());
            }
            let rows = repo.slow_queries(limit).await?;
            print_json(&rows)
        }
        InspectAction::Config => {
            let cfg = ServerConfig::from_env();
            print_json(&serde_json::json!({
                "request_timeout_secs": cfg.request_timeout.as_secs(),
                "max_body_bytes": cfg.max_body_bytes,
                "lab_enabled": cfg.lab_enabled,
                "lab_sql_timeout_ms_max": cfg.lab_sql_timeout_ms_max,
                "lab_sql_row_cap_max": cfg.lab_sql_row_cap_max,
            }))
        }
    }
}
