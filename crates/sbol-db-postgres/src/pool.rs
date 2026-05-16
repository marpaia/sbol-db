use std::time::Duration;

use sbol_db_core::DomainError;

pub use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

fn db_err<E: std::fmt::Display>(e: E) -> DomainError {
    DomainError::Database(e.to_string())
}

/// Connection pool configuration. Sensible defaults match the prior
/// hardcoded behavior (max_connections=8, sqlx defaults elsewhere); each
/// field can be overridden by an environment variable so the Helm chart
/// can tune pool sizing without code changes.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout: Duration,
    pub idle_timeout: Option<Duration>,
    pub max_lifetime: Option<Duration>,
    pub connect_timeout: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 8,
            min_connections: 0,
            acquire_timeout: Duration::from_secs(5),
            idle_timeout: Some(Duration::from_secs(300)),
            max_lifetime: Some(Duration::from_secs(1800)),
            connect_timeout: Duration::from_secs(5),
        }
    }
}

impl PoolConfig {
    /// Read overrides from environment variables. Variables that fail to
    /// parse fall back to the default rather than aborting startup; the
    /// goal is graceful degradation, not strict validation. A value of 0
    /// disables the `idle_timeout` / `max_lifetime` knobs.
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            max_connections: env_u32("DATABASE_MAX_CONNECTIONS", defaults.max_connections),
            min_connections: env_u32("DATABASE_MIN_CONNECTIONS", defaults.min_connections),
            acquire_timeout: env_duration_secs(
                "DATABASE_ACQUIRE_TIMEOUT_SECS",
                defaults.acquire_timeout,
            ),
            idle_timeout: env_opt_duration_secs(
                "DATABASE_IDLE_TIMEOUT_SECS",
                defaults.idle_timeout,
            ),
            max_lifetime: env_opt_duration_secs(
                "DATABASE_MAX_LIFETIME_SECS",
                defaults.max_lifetime,
            ),
            connect_timeout: env_duration_secs(
                "DATABASE_CONNECT_TIMEOUT_SECS",
                defaults.connect_timeout,
            ),
        }
    }
}

/// One-shot connect. Suitable for short-lived CLI commands where a fast
/// failure beats a long retry loop. Reads pool sizing from env.
pub async fn connect(database_url: &str) -> Result<PgPool, DomainError> {
    connect_with_config(database_url, &PoolConfig::from_env()).await
}

pub async fn connect_with_config(
    database_url: &str,
    cfg: &PoolConfig,
) -> Result<PgPool, DomainError> {
    let mut opts = sqlx::postgres::PgPoolOptions::new()
        .max_connections(cfg.max_connections)
        .min_connections(cfg.min_connections)
        .acquire_timeout(cfg.acquire_timeout);
    if let Some(t) = cfg.idle_timeout {
        opts = opts.idle_timeout(t);
    }
    if let Some(t) = cfg.max_lifetime {
        opts = opts.max_lifetime(t);
    }
    tokio::time::timeout(cfg.connect_timeout, opts.connect(database_url))
        .await
        .map_err(|_| DomainError::Database("connect timed out".into()))?
        .map_err(db_err)
}

/// Retrying connect for boot-time use. Retries with capped exponential
/// backoff (1s → 2s → 4s → … → 10s) until `total_deadline` elapses,
/// then returns the last error. Use this from the `serve` and
/// `migrate up` paths where Postgres may not be ready when sbol-db
/// starts (the bitnami subchart's postgres pod boots in parallel with
/// the chart's migration Job).
pub async fn connect_with_retry(
    database_url: &str,
    total_deadline: Duration,
) -> Result<PgPool, DomainError> {
    let cfg = PoolConfig::from_env();
    let started = std::time::Instant::now();
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(10);
    loop {
        match connect_with_config(database_url, &cfg).await {
            Ok(pool) => return Ok(pool),
            Err(err) => {
                let remaining = total_deadline.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    return Err(err);
                }
                let sleep_for = backoff.min(remaining);
                tracing::warn!(
                    error = %err,
                    retry_in_secs = sleep_for.as_secs_f64(),
                    "database connect failed; retrying",
                );
                tokio::time::sleep(sleep_for).await;
                backoff = (backoff * 2).min(max_backoff);
            }
        }
    }
}

pub async fn run_migrations(pool: &PgPool) -> Result<(), DomainError> {
    MIGRATOR.run(pool).await.map_err(db_err)
}

pub async fn migration_status(pool: &PgPool) -> Result<Vec<MigrationEntry>, DomainError> {
    let mut conn = pool.acquire().await.map_err(db_err)?;
    let applied = MIGRATOR
        .iter()
        .map(|m| (m.version, m.description.to_string()))
        .collect::<Vec<_>>();
    let applied_versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut *conn)
            .await
            .unwrap_or_default();
    Ok(applied
        .into_iter()
        .map(|(v, d)| MigrationEntry {
            version: v,
            description: d,
            applied: applied_versions.contains(&v),
        })
        .collect())
}

#[derive(Debug, Clone)]
pub struct MigrationEntry {
    pub version: i64,
    pub description: String,
    pub applied: bool,
}

fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn env_duration_secs(name: &str, default: Duration) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(default)
}

fn env_opt_duration_secs(name: &str, default: Option<Duration>) -> Option<Duration> {
    match std::env::var(name).ok().and_then(|s| s.parse::<u64>().ok()) {
        Some(0) => None, // explicit disable
        Some(n) => Some(Duration::from_secs(n)),
        None => default,
    }
}
