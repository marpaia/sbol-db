//! `/lab/api/observability/*` — in-app observability for the lab UI.
//!
//! Two surfaces:
//!
//!  - **Summary** (`/summary`) returns a compact JSON blob covering
//!    process health, pool capacity, job queue state, and a rolling
//!    10-minute traffic window. The lab UI polls this every ~5 seconds.
//!  - **Postgres** (`/postgres/*`) wraps the catalog views the database
//!    maintenance page renders. Each handler is one bounded query; the
//!    slow-queries endpoint detects-and-skips when
//!    `pg_stat_statements` isn't installed.
//!
//! All routes are read-only. Polling the summary at 5 s only adds two
//! catalog queries (queue depth + oldest age) per call; the rolling
//! traffic snapshot is in-memory and free.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use sbol_db_postgres::{
    Activity, BlockingLock, DatabaseSize, IndexStats, PgStatsRepository, SlowQuery, TableSchema,
    TableStats,
};
use sbol_db_storage::{JobStatus, ListJobsFilter, OldestQueuedAge, QueueDepthRow, SbolJob};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::error::ApiError;
use crate::metrics::{rolling_snapshot, uptime_secs, PoolSnapshot, RollingSnapshot};
use crate::AppState;

const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/summary", get(summary))
        .route("/postgres/database", get(pg_database))
        .route("/postgres/tables", get(pg_tables))
        .route("/postgres/indexes", get(pg_indexes))
        .route("/postgres/activity", get(pg_activity))
        .route("/postgres/locks", get(pg_locks))
        .route("/postgres/tables/:name/schema", get(pg_table_schema))
        .route("/postgres/slow-queries", get(pg_slow_queries))
        .route("/jobs/recent", get(jobs_recent))
}

// ---------- Summary -----------------------------------------------------------

#[derive(Serialize)]
pub struct Summary {
    pub health: Health,
    pub pool: PoolSnapshot,
    pub jobs: JobsSnapshot,
    pub rolling: RollingSnapshot,
}

#[derive(Serialize)]
pub struct Health {
    pub ready: bool,
    pub version: &'static str,
    pub uptime_secs: u64,
    pub snapshot_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct JobsSnapshot {
    pub queue_depth: Vec<QueueDepthRowJson>,
    pub oldest_age: Vec<OldestQueuedAgeJson>,
    pub failures_24h: i64,
}

#[derive(Serialize)]
pub struct QueueDepthRowJson {
    pub status: &'static str,
    pub queue: String,
    pub count: i64,
}

#[derive(Serialize)]
pub struct OldestQueuedAgeJson {
    pub queue: String,
    pub age_secs: f64,
}

pub async fn summary(State(state): State<AppState>) -> Result<Json<Summary>, ApiError> {
    let ready = state.service.ping().await.is_ok();

    let queue_depth: Vec<QueueDepthRowJson> = state
        .jobs
        .queue_depth_snapshot()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r: QueueDepthRow| QueueDepthRowJson {
            status: r.status.as_db_str(),
            queue: r.queue,
            count: r.count,
        })
        .collect();

    let oldest_age: Vec<OldestQueuedAgeJson> = state
        .jobs
        .oldest_queued_age()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r: OldestQueuedAge| OldestQueuedAgeJson {
            queue: r.queue,
            age_secs: r.age_secs,
        })
        .collect();

    // Postgres-only refinement; other backends report 0 here (the rest of the
    // summary is backend-neutral and still rendered).
    let failures_24h: i64 = match &state.pg_pool {
        Some(pool) => sqlx::query(
            r#"
            SELECT COUNT(*)::bigint AS n
            FROM sbol_jobs
            WHERE status IN ('failed', 'dead')
              AND finished_at >= now() - interval '24 hours'
            "#,
        )
        .fetch_one(pool)
        .await
        .map(|r| r.try_get::<i64, _>("n").unwrap_or(0))
        .unwrap_or(0),
        None => 0,
    };

    Ok(Json(Summary {
        health: Health {
            ready,
            version: SERVER_VERSION,
            uptime_secs: uptime_secs(),
            snapshot_at: Utc::now(),
        },
        pool: state.metrics.pool_snapshot(),
        jobs: JobsSnapshot {
            queue_depth,
            oldest_age,
            failures_24h,
        },
        rolling: rolling_snapshot(),
    }))
}

// ---------- Postgres ----------------------------------------------------------

fn pg_repo(state: &AppState) -> Result<PgStatsRepository, ApiError> {
    Ok(PgStatsRepository::new(state.require_pg_pool()?.clone()))
}

pub async fn pg_database(State(state): State<AppState>) -> Result<Json<DatabaseSize>, ApiError> {
    Ok(Json(pg_repo(&state)?.database_size().await?))
}

#[derive(Deserialize)]
pub struct PgPageQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub async fn pg_tables(
    State(state): State<AppState>,
    Query(params): Query<PgPageQuery>,
) -> Result<Json<Vec<TableStats>>, ApiError> {
    let limit = params.limit.unwrap_or(20).clamp(1, 200);
    let offset = params.offset.unwrap_or(0).max(0);
    Ok(Json(pg_repo(&state)?.table_stats(limit, offset).await?))
}

pub async fn pg_indexes(
    State(state): State<AppState>,
    Query(params): Query<PgPageQuery>,
) -> Result<Json<Vec<IndexStats>>, ApiError> {
    let limit = params.limit.unwrap_or(30).clamp(1, 200);
    Ok(Json(pg_repo(&state)?.index_stats(limit).await?))
}

#[derive(Deserialize)]
pub struct ActivityQuery {
    pub limit: Option<i64>,
}

pub async fn pg_activity(
    State(state): State<AppState>,
    Query(params): Query<ActivityQuery>,
) -> Result<Json<Vec<Activity>>, ApiError> {
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    Ok(Json(pg_repo(&state)?.activity(limit).await?))
}

pub async fn pg_locks(State(state): State<AppState>) -> Result<Json<Vec<BlockingLock>>, ApiError> {
    Ok(Json(pg_repo(&state)?.blocking_locks().await?))
}

pub async fn pg_table_schema(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<TableSchema>, ApiError> {
    pg_repo(&state)?
        .table_schema(&name)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("relation {name}")))
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SlowQueriesResponse {
    NotInstalled { setup_hint: &'static str },
    Installed { rows: Vec<SlowQuery> },
}

const SLOW_QUERIES_SETUP_HINT: &str = concat!(
    "Enable pg_stat_statements:\n",
    "  1. Add `shared_preload_libraries = 'pg_stat_statements'` to postgresql.conf\n",
    "  2. Restart Postgres\n",
    "  3. Run `CREATE EXTENSION pg_stat_statements;` as a superuser",
);

pub async fn pg_slow_queries(
    State(state): State<AppState>,
    Query(params): Query<PgPageQuery>,
) -> Result<Json<SlowQueriesResponse>, ApiError> {
    let repo = pg_repo(&state)?;
    if !repo.has_pg_stat_statements().await? {
        return Ok(Json(SlowQueriesResponse::NotInstalled {
            setup_hint: SLOW_QUERIES_SETUP_HINT,
        }));
    }
    let limit = params.limit.unwrap_or(20).clamp(1, 100);
    let rows = repo.slow_queries(limit).await?;
    Ok(Json(SlowQueriesResponse::Installed { rows }))
}

// ---------- Recent jobs -------------------------------------------------------

#[derive(Deserialize)]
pub struct JobsRecentQuery {
    pub limit: Option<u32>,
    pub queue: Option<String>,
    pub status: Option<String>,
}

pub async fn jobs_recent(
    State(state): State<AppState>,
    Query(params): Query<JobsRecentQuery>,
) -> Result<Json<Vec<SbolJob>>, ApiError> {
    let limit = params.limit.unwrap_or(50).clamp(1, 500);
    let status = match params.status.as_deref() {
        Some(s) if !s.is_empty() => Some(
            JobStatus::from_db_str(s)
                .map_err(|e| ApiError::BadRequest(format!("invalid status: {e}")))?,
        ),
        _ => None,
    };
    let filter = ListJobsFilter {
        kind: None,
        status,
        queue: params.queue.filter(|s| !s.is_empty()),
        correlation_id: None,
        since: None,
        limit,
    };
    Ok(Json(state.jobs.list(&filter).await?))
}
