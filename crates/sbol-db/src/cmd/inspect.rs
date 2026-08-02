//! `sbol-db inspect` — read-only database introspection via the backend's
//! [`DbStats`] capability, or printing the effective `ServerConfig`. Output is
//! always JSON.

use std::sync::Arc;

use anyhow::Result;
use sbol_db_server::{CorsPolicy, ServerConfig};
use sbol_db_storage::DbStats;

use crate::cli::InspectAction;
use crate::output::print_json;

pub async fn run(stats: Arc<dyn DbStats>, action: InspectAction) -> Result<()> {
    match action {
        InspectAction::Size => {
            let size = stats.database_size().await?;
            print_json(&size)
        }
        InspectAction::Tables { limit, offset } => {
            let rows = stats.table_stats(limit, offset).await?;
            print_json(&rows)
        }
        InspectAction::Table { name } => {
            let schema = stats.table_schema(&name).await?;
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
            let rows = stats.activity(limit).await?;
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
            let rows = stats.blocking_locks().await?;
            print_json(&rows)
        }
        InspectAction::Indexes { limit } => {
            let rows = stats.index_stats(limit).await?;
            print_json(&rows)
        }
        InspectAction::SlowQueries { limit } => {
            if !stats.has_slow_query_stats().await? {
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
            let rows = stats.slow_queries(limit).await?;
            print_json(&rows)
        }
        InspectAction::Config => {
            let cfg = ServerConfig::from_env();
            let cors = match &cfg.cors {
                CorsPolicy::Permissive => serde_json::json!({ "mode": "permissive" }),
                CorsPolicy::SameOrigin => serde_json::json!({ "mode": "same_origin" }),
                CorsPolicy::AllowList(origins) => serde_json::json!({
                    "mode": "allowlist",
                    "origins": origins
                        .iter()
                        .filter_map(|origin| origin.to_str().ok())
                        .collect::<Vec<_>>(),
                }),
            };
            print_json(&serde_json::json!({
                "request_timeout_secs": cfg.request_timeout.as_secs(),
                "max_body_bytes": cfg.max_body_bytes,
                "lab_enabled": cfg.lab_enabled,
                "portal_enabled": cfg.portal_enabled,
                "session_cookie_secure": cfg.session_cookie_secure,
                "admin_api_auth_required": cfg.admin_api_auth_required,
                "lab_sql_timeout_ms_max": cfg.lab_sql_timeout_ms_max,
                "lab_sql_row_cap_max": cfg.lab_sql_row_cap_max,
                "allow_public_signup": cfg.allow_public_signup,
                "sparql_write_enabled": cfg.sparql_write_enabled,
                "https_security_headers": cfg.https_security_headers,
                "cors": cors,
                "setup_token_configured": cfg.setup_token_hash.is_some(),
            }))
        }
    }
}
