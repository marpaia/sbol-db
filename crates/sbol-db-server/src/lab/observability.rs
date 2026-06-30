//! `/lab/api/observability/*` — in-app observability for the lab UI.
//!
//! Three surfaces:
//!
//!  - **Summary** (`/summary`) returns a compact JSON blob covering process
//!    health, pool capacity, job queue state, and a rolling 10-minute traffic
//!    window. The lab UI polls this every ~5 seconds. Every field is
//!    backend-neutral.
//!  - **Maintenance** (`/maintenance/*`) is the engine-internals surface. For
//!    a relational engine (Postgres, SQLite) it reports tables, indexes,
//!    schema, and — on Postgres — sessions, locks, and slow queries. For an
//!    LSM key-value store (RocksDB) it reports column families, levels, and
//!    compaction. Each handler requires the matching capability and answers a
//!    clear "backend unsupported" error on a backend that lacks it.
//!  - **Recent jobs** (`/jobs/recent`) lists the job queue, on every backend.
//!
//! All read routes are bounded single queries; the two write routes
//! (`optimize`, `compact`) trigger the engine's own maintenance.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use sbol_db_storage::{
    Activity, BlockingLock, DatabaseSize, IndexStats, JobStatus, ListJobsFilter, LsmOverview,
    OldestQueuedAge, QueueDepthRow, SbolJob, SlowQuery, TableSchema, TableStats,
};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::metrics::{rolling_snapshot, uptime_secs, PoolSnapshot, RollingSnapshot};
use crate::AppState;

const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/summary", get(summary))
        // Relational-engine maintenance (Postgres, SQLite).
        .route("/maintenance/database", get(maint_database))
        .route("/maintenance/tables", get(maint_tables))
        .route("/maintenance/indexes", get(maint_indexes))
        .route("/maintenance/activity", get(maint_activity))
        .route("/maintenance/locks", get(maint_locks))
        .route("/maintenance/tables/:name/schema", get(maint_table_schema))
        .route("/maintenance/slow-queries", get(maint_slow_queries))
        .route("/maintenance/optimize", post(maint_optimize))
        // LSM-engine maintenance (RocksDB).
        .route("/maintenance/lsm", get(lsm_overview))
        .route("/maintenance/compact", post(lsm_compact))
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

    // Backend-neutral: every job queue can count its own recent failures.
    let failures_24h = state.jobs.recent_failure_count(24).await.unwrap_or(0);

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

// ---------- Relational-engine maintenance -------------------------------------

#[derive(Deserialize)]
pub struct PageQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub async fn maint_database(State(state): State<AppState>) -> Result<Json<DatabaseSize>, ApiError> {
    Ok(Json(state.require_db_stats()?.database_size().await?))
}

pub async fn maint_tables(
    State(state): State<AppState>,
    Query(params): Query<PageQuery>,
) -> Result<Json<Vec<TableStats>>, ApiError> {
    let limit = params.limit.unwrap_or(20).clamp(1, 200);
    let offset = params.offset.unwrap_or(0).max(0);
    Ok(Json(
        state.require_db_stats()?.table_stats(limit, offset).await?,
    ))
}

pub async fn maint_indexes(
    State(state): State<AppState>,
    Query(params): Query<PageQuery>,
) -> Result<Json<Vec<IndexStats>>, ApiError> {
    let limit = params.limit.unwrap_or(30).clamp(1, 200);
    Ok(Json(state.require_db_stats()?.index_stats(limit).await?))
}

#[derive(Deserialize)]
pub struct ActivityQuery {
    pub limit: Option<i64>,
}

pub async fn maint_activity(
    State(state): State<AppState>,
    Query(params): Query<ActivityQuery>,
) -> Result<Json<Vec<Activity>>, ApiError> {
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    Ok(Json(state.require_db_stats()?.activity(limit).await?))
}

pub async fn maint_locks(
    State(state): State<AppState>,
) -> Result<Json<Vec<BlockingLock>>, ApiError> {
    Ok(Json(state.require_db_stats()?.blocking_locks().await?))
}

pub async fn maint_table_schema(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<TableSchema>, ApiError> {
    state
        .require_db_stats()?
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

pub async fn maint_slow_queries(
    State(state): State<AppState>,
    Query(params): Query<PageQuery>,
) -> Result<Json<SlowQueriesResponse>, ApiError> {
    let stats = state.require_db_stats()?;
    if !stats.has_slow_query_stats().await? {
        return Ok(Json(SlowQueriesResponse::NotInstalled {
            setup_hint: SLOW_QUERIES_SETUP_HINT,
        }));
    }
    let limit = params.limit.unwrap_or(20).clamp(1, 100);
    let rows = stats.slow_queries(limit).await?;
    Ok(Json(SlowQueriesResponse::Installed { rows }))
}

#[derive(Serialize)]
pub struct ActionResult {
    pub ok: bool,
}

pub async fn maint_optimize(State(state): State<AppState>) -> Result<Json<ActionResult>, ApiError> {
    state.require_db_stats()?.optimize().await?;
    Ok(Json(ActionResult { ok: true }))
}

// ---------- LSM-engine maintenance --------------------------------------------

pub async fn lsm_overview(State(state): State<AppState>) -> Result<Json<LsmOverview>, ApiError> {
    Ok(Json(state.require_lsm_stats()?.overview().await?))
}

pub async fn lsm_compact(State(state): State<AppState>) -> Result<Json<ActionResult>, ApiError> {
    state.require_lsm_stats()?.compact().await?;
    Ok(Json(ActionResult { ok: true }))
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
