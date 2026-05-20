//! `sbol-db db` — migrations and composite health check.

use anyhow::Result;
use sbol_db_jobs::default_registry;
use sbol_db_postgres::{JobRepository, PgPool, SbolObjectService};
use serde::Serialize;

use crate::cli::DbAction;

pub async fn run(pool: PgPool, action: DbAction) -> Result<()> {
    match action {
        DbAction::Migrate => {
            sbol_db_postgres::run_migrations(&pool).await?;
            println!("migrations applied");
            Ok(())
        }
        DbAction::MigrateStatus => {
            let entries = sbol_db_postgres::pool::migration_status(&pool).await?;
            for entry in entries {
                let marker = if entry.applied { "[x]" } else { "[ ]" };
                println!("{marker} {} {}", entry.version, entry.description);
            }
            Ok(())
        }
        DbAction::Doctor {
            json,
            require_ontologies,
            max_queued_age_secs,
        } => doctor(pool, json, require_ontologies, max_queued_age_secs).await,
    }
}

#[derive(Debug, Clone, Serialize)]
struct CheckOutcome {
    name: &'static str,
    ok: bool,
    detail: String,
}

#[derive(Debug, Serialize)]
struct Report {
    ok: bool,
    checks: Vec<CheckOutcome>,
}

async fn doctor(
    pool: PgPool,
    json: bool,
    require_ontologies: String,
    max_queued_age_secs: i64,
) -> Result<()> {
    let mut report = Report {
        ok: true,
        checks: Vec::new(),
    };

    // 1. DB connectivity: a trivial SELECT so we never claim healthy on a stale pool.
    let connect_ok = sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&pool)
        .await
        .is_ok();
    push(
        &mut report,
        "database",
        connect_ok,
        if connect_ok {
            "SELECT 1 succeeded".to_owned()
        } else {
            "SELECT 1 failed".to_owned()
        },
    );

    // 2. Migrations: every entry must be applied.
    match sbol_db_postgres::pool::migration_status(&pool).await {
        Ok(entries) => {
            let pending: Vec<String> = entries
                .iter()
                .filter(|e| !e.applied)
                .map(|e| format!("{} {}", e.version, e.description))
                .collect();
            let ok = pending.is_empty();
            let detail = if ok {
                format!("{} migrations applied", entries.len())
            } else {
                format!("{} pending: {}", pending.len(), pending.join(", "))
            };
            push(&mut report, "migrations", ok, detail);
        }
        Err(e) => push(&mut report, "migrations", false, format!("{e}")),
    }

    // 3. Worker registry: each registered kind looks up to a handler reporting the same kind.
    let registry = default_registry();
    let kinds: Vec<&'static str> = registry.kinds().collect();
    let mut bad: Vec<&'static str> = Vec::new();
    for k in &kinds {
        match registry.lookup(k) {
            Some(h) if h.kind() == *k => {}
            _ => bad.push(k),
        }
    }
    let ok = bad.is_empty();
    let detail = if ok {
        format!("{} kinds registered: {}", kinds.len(), kinds.join(", "))
    } else {
        format!("misregistered kinds: {}", bad.join(", "))
    };
    push(&mut report, "worker_registry", ok, detail);

    // 4. Queue health.
    let jobs = JobRepository::new(pool.clone());
    let budget = max_queued_age_secs as f64;
    match jobs.oldest_queued_age().await {
        Ok(ages) => {
            let too_old: Vec<(String, f64)> = ages
                .iter()
                .filter(|row| row.age_secs > budget)
                .map(|row| (row.queue.clone(), row.age_secs))
                .collect();
            let ok = too_old.is_empty();
            let detail = if ok {
                if ages.is_empty() {
                    "no queued jobs".to_owned()
                } else {
                    let parts: Vec<String> = ages
                        .iter()
                        .map(|r| format!("{}={:.0}s", r.queue, r.age_secs))
                        .collect();
                    format!("oldest age: {}", parts.join(", "))
                }
            } else {
                let parts: Vec<String> = too_old
                    .iter()
                    .map(|(q, s)| format!("{q}: {s:.0}s"))
                    .collect();
                format!("queue starvation: {}", parts.join(", "))
            };
            push(&mut report, "queue_health", ok, detail);
        }
        Err(e) => push(&mut report, "queue_health", false, format!("{e}")),
    }

    // 5. Required ontologies.
    let want: Vec<String> = require_ontologies
        .split(',')
        .map(|s| s.trim().to_ascii_uppercase())
        .filter(|s| !s.is_empty())
        .collect();
    if !want.is_empty() {
        let service = SbolObjectService::new(pool.clone());
        match service.ontology().list_ontologies().await {
            Ok(rows) => {
                let have: std::collections::HashSet<String> =
                    rows.iter().map(|r| r.prefix.to_ascii_uppercase()).collect();
                let missing: Vec<String> = want
                    .iter()
                    .filter(|p| !have.contains(*p))
                    .cloned()
                    .collect();
                let ok = missing.is_empty();
                let detail = if ok {
                    format!("required ontologies present: {}", want.join(", "))
                } else {
                    format!("missing ontologies: {}", missing.join(", "))
                };
                push(&mut report, "ontologies", ok, detail);
            }
            Err(e) => push(&mut report, "ontologies", false, format!("{e}")),
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for c in &report.checks {
            let marker = if c.ok { "[ok]" } else { "[FAIL]" };
            println!("{marker} {}: {}", c.name, c.detail);
        }
        if report.ok {
            println!("\noverall: ok");
        } else {
            println!("\noverall: FAIL");
        }
    }

    if !report.ok {
        std::process::exit(1);
    }
    Ok(())
}

fn push(report: &mut Report, name: &'static str, ok: bool, detail: String) {
    if !ok {
        report.ok = false;
    }
    report.checks.push(CheckOutcome { name, ok, detail });
}
