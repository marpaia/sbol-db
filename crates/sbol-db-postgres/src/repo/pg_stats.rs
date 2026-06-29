//! Postgres maintenance/observability views.
//!
//! Thin wrappers around the `pg_stat_*`, `pg_locks`, and `pg_stat_statements`
//! catalog views that the lab UI's maintenance page reads on Postgres. The
//! queries are intentionally small and bounded — these read-only handlers are
//! polled every ~15 seconds by an open browser tab, so each method does at
//! most a single index-friendly catalog scan.
//!
//! sbol-db doesn't use Postgres schemas as a domain concept (no tenancy /
//! isolation surface in the UI), so every table-shaped query here filters
//! to `schemaname = 'public'`. If we ever introduce a real multi-schema
//! verb, lift this filter and re-introduce the schema parameter.
//!
//! `pg_stat_statements` is optional: callers should check
//! [`PgStatsRepository::has_pg_stat_statements`] before calling
//! [`PgStatsRepository::slow_queries`].

/// Hard-coded user schema. The whole codebase writes into `public`; the
/// pg_stats helpers filter to it so the lab UI doesn't surface stray
/// extension-owned schemas (or the catalog's own).
const USER_SCHEMA: &str = "public";

use async_trait::async_trait;
use sbol_db_core::DomainError;
use sbol_db_storage::DbStats;
use sqlx::Row;

pub use sbol_db_storage::{
    Activity, BlockingLock, DatabaseSize, IncomingForeignKey, IndexStats, OutgoingForeignKey,
    RelationalColumn, RelationalSchema, RelationalTable, SlowQuery, TableColumn, TableSchema,
    TableStats,
};

use crate::repo::db_err;
use crate::PgPool;

#[derive(Clone)]
pub struct PgStatsRepository {
    pool: PgPool,
}

impl PgStatsRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Every table in the user schema with its columns, for the lab's schema
    /// browser and SQL-editor click-to-insert sidebar. `_sqlx_migrations` is
    /// sqlx's own bookkeeping table and is filtered out.
    pub async fn schema_overview(&self) -> Result<RelationalSchema, DomainError> {
        let rows = sqlx::query(
            r#"
            SELECT table_name, column_name, data_type, udt_name, is_nullable, ordinal_position
            FROM information_schema.columns
            WHERE table_schema = $1
              AND table_name <> '_sqlx_migrations'
            ORDER BY table_name, ordinal_position
            "#,
        )
        .bind(USER_SCHEMA)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        let mut current: Option<RelationalTable> = None;
        let mut tables: Vec<RelationalTable> = Vec::new();
        for row in rows {
            let table_name: String = row.try_get("table_name").map_err(db_err)?;
            let column_name: String = row.try_get("column_name").map_err(db_err)?;
            let data_type: String = row.try_get("data_type").map_err(db_err)?;
            let udt_name: String = row.try_get("udt_name").map_err(db_err)?;
            let is_nullable: String = row.try_get("is_nullable").map_err(db_err)?;

            // information_schema reports `USER-DEFINED`/`ARRAY` for domains and
            // arrays; the underlying udt name is the useful label.
            let column_type = if data_type == "USER-DEFINED" || data_type == "ARRAY" {
                udt_name
            } else {
                data_type
            };
            let column = RelationalColumn {
                name: column_name,
                column_type,
                nullable: is_nullable == "YES",
            };

            match &mut current {
                Some(t) if t.name == table_name => t.columns.push(column),
                _ => {
                    if let Some(prev) = current.take() {
                        tables.push(prev);
                    }
                    current = Some(RelationalTable {
                        name: table_name,
                        columns: vec![column],
                    });
                }
            }
        }
        if let Some(last) = current.take() {
            tables.push(last);
        }

        Ok(RelationalSchema { tables })
    }

    pub async fn database_size(&self) -> Result<DatabaseSize, DomainError> {
        let row = sqlx::query(
            r#"
            SELECT current_database()::text                          AS db,
                   pg_database_size(current_database())::bigint      AS bytes
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(DatabaseSize {
            database: row.try_get("db").map_err(db_err)?,
            total_bytes: row.try_get("bytes").map_err(db_err)?,
        })
    }

    pub async fn table_stats(
        &self,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<TableStats>, DomainError> {
        let rows = sqlx::query(
            r#"
            SELECT
                s.relname::text                             AS name,
                COALESCE(c.reltuples, 0)::bigint            AS rows_estimate,
                pg_total_relation_size(c.oid)::bigint       AS total_bytes,
                pg_indexes_size(c.oid)::bigint              AS index_bytes,
                s.n_live_tup::bigint                        AS n_live_tup,
                s.n_dead_tup::bigint                        AS n_dead_tup,
                s.last_vacuum,
                s.last_autovacuum,
                s.last_analyze
            FROM pg_stat_user_tables s
            JOIN pg_class c ON c.oid = s.relid
            WHERE s.schemaname = $1
            ORDER BY pg_total_relation_size(c.oid) DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(USER_SCHEMA)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.into_iter()
            .map(|r| {
                Ok::<_, DomainError>(TableStats {
                    name: r.try_get("name").map_err(db_err)?,
                    rows_estimate: r.try_get("rows_estimate").map_err(db_err)?,
                    total_bytes: r.try_get("total_bytes").map_err(db_err)?,
                    index_bytes: r.try_get("index_bytes").map_err(db_err)?,
                    n_live_tup: r.try_get("n_live_tup").map_err(db_err)?,
                    n_dead_tup: r.try_get("n_dead_tup").map_err(db_err)?,
                    last_vacuum: r.try_get("last_vacuum").map_err(db_err)?,
                    last_autovacuum: r.try_get("last_autovacuum").map_err(db_err)?,
                    last_analyze: r.try_get("last_analyze").map_err(db_err)?,
                })
            })
            .collect()
    }

    pub async fn index_stats(&self, limit: i64) -> Result<Vec<IndexStats>, DomainError> {
        let rows = sqlx::query(
            r#"
            SELECT
                s.relname::text                        AS table_name,
                s.indexrelname::text                   AS index_name,
                COALESCE(s.idx_scan, 0)::bigint        AS idx_scan,
                pg_relation_size(s.indexrelid)::bigint AS bytes
            FROM pg_stat_user_indexes s
            WHERE s.schemaname = $1
            ORDER BY pg_relation_size(s.indexrelid) DESC
            LIMIT $2
            "#,
        )
        .bind(USER_SCHEMA)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.into_iter()
            .map(|r| {
                Ok::<_, DomainError>(IndexStats {
                    table: r.try_get("table_name").map_err(db_err)?,
                    index: r.try_get("index_name").map_err(db_err)?,
                    idx_scan: r.try_get("idx_scan").map_err(db_err)?,
                    bytes: r.try_get("bytes").map_err(db_err)?,
                })
            })
            .collect()
    }

    pub async fn activity(&self, limit: i64) -> Result<Vec<Activity>, DomainError> {
        let rows = sqlx::query(
            r#"
            SELECT
                pid,
                application_name,
                state,
                wait_event_type,
                wait_event,
                query_start,
                EXTRACT(EPOCH FROM (now() - query_start))::float8 AS duration_secs,
                LEFT(query, 500) AS query,
                client_addr::text AS client_addr
            FROM pg_stat_activity
            WHERE pid <> pg_backend_pid()
              AND state IS NOT NULL
              AND query IS NOT NULL
              AND query != ''
              AND backend_type = 'client backend'
            ORDER BY query_start ASC NULLS LAST
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.into_iter()
            .map(|r| {
                Ok::<_, DomainError>(Activity {
                    pid: r.try_get("pid").map_err(db_err)?,
                    application_name: r.try_get("application_name").map_err(db_err)?,
                    state: r.try_get("state").map_err(db_err)?,
                    wait_event_type: r.try_get("wait_event_type").map_err(db_err)?,
                    wait_event: r.try_get("wait_event").map_err(db_err)?,
                    query: r.try_get("query").map_err(db_err)?,
                    query_start: r.try_get("query_start").map_err(db_err)?,
                    duration_secs: r.try_get("duration_secs").map_err(db_err)?,
                    client_addr: r.try_get("client_addr").map_err(db_err)?,
                })
            })
            .collect()
    }

    pub async fn blocking_locks(&self) -> Result<Vec<BlockingLock>, DomainError> {
        let rows = sqlx::query(
            r#"
            SELECT
                kl.pid           AS blocker_pid,
                LEFT(ka.query, 500) AS blocker_query,
                bl.pid           AS blocked_pid,
                LEFT(ba.query, 500) AS blocked_query,
                bl.mode,
                bl.locktype
            FROM pg_locks bl
            JOIN pg_stat_activity ba ON ba.pid = bl.pid
            JOIN pg_locks kl
                ON kl.locktype = bl.locktype
               AND kl.granted
               AND ((bl.relation = kl.relation AND bl.relation IS NOT NULL)
                    OR (bl.objid = kl.objid AND bl.objid IS NOT NULL)
                    OR (bl.transactionid = kl.transactionid AND bl.transactionid IS NOT NULL))
               AND bl.pid <> kl.pid
            JOIN pg_stat_activity ka ON ka.pid = kl.pid
            WHERE NOT bl.granted
            ORDER BY blocked_pid
            LIMIT 50
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.into_iter()
            .map(|r| {
                Ok::<_, DomainError>(BlockingLock {
                    blocker_pid: r.try_get("blocker_pid").map_err(db_err)?,
                    blocker_query: r.try_get("blocker_query").map_err(db_err)?,
                    blocked_pid: r.try_get("blocked_pid").map_err(db_err)?,
                    blocked_query: r.try_get("blocked_query").map_err(db_err)?,
                    mode: r.try_get("mode").map_err(db_err)?,
                    locktype: r.try_get("locktype").map_err(db_err)?,
                })
            })
            .collect()
    }

    /// Returns true when the `pg_stat_statements` extension is installed.
    /// Cheap (a single index lookup against `pg_extension`); callers should
    /// check this before calling [`Self::slow_queries`].
    pub async fn has_pg_stat_statements(&self) -> Result<bool, DomainError> {
        let row = sqlx::query(
            r#"
            SELECT EXISTS(
              SELECT 1 FROM pg_extension WHERE extname = 'pg_stat_statements'
            ) AS present
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        row.try_get::<bool, _>("present").map_err(db_err)
    }

    /// Full schema introspection for a single relation. Returns `None`
    /// when the table doesn't exist (or isn't a relation/matview/
    /// partitioned table). Each query is one indexed catalog lookup,
    /// fine for an on-demand drill-down view.
    pub async fn table_schema(&self, name: &str) -> Result<Option<TableSchema>, DomainError> {
        let exists_row = sqlx::query(
            r#"
            SELECT obj_description(c.oid, 'pg_class') AS comment
            FROM pg_class c
            JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE n.nspname = $1
              AND c.relname = $2
              AND c.relkind IN ('r','m','p')
            "#,
        )
        .bind(USER_SCHEMA)
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        let Some(exists_row) = exists_row else {
            return Ok(None);
        };
        let comment: Option<String> = exists_row.try_get("comment").map_err(db_err)?;

        let column_rows = sqlx::query(
            r#"
            WITH tbl AS (
              SELECT c.oid
              FROM pg_class c
              JOIN pg_namespace n ON n.oid = c.relnamespace
              WHERE n.nspname = $1 AND c.relname = $2
            ),
            pk AS (
              SELECT unnest(con.conkey) AS attnum
              FROM pg_constraint con
              JOIN tbl ON con.conrelid = tbl.oid
              WHERE con.contype = 'p'
            )
            SELECT
              a.attname::text                                        AS name,
              pg_catalog.format_type(a.atttypid, a.atttypmod)::text  AS column_type,
              (NOT a.attnotnull)                                     AS nullable,
              pg_get_expr(d.adbin, d.adrelid)                        AS default_expr,
              a.attnum::int                                          AS ordinal,
              pg_catalog.col_description(a.attrelid, a.attnum)       AS comment,
              EXISTS (SELECT 1 FROM pk WHERE pk.attnum = a.attnum)   AS is_primary_key
            FROM tbl
            JOIN pg_attribute a ON a.attrelid = tbl.oid
            LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
            WHERE a.attnum > 0
              AND NOT a.attisdropped
            ORDER BY a.attnum
            "#,
        )
        .bind(USER_SCHEMA)
        .bind(name)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        let columns: Vec<TableColumn> = column_rows
            .into_iter()
            .map(|r| {
                Ok::<_, DomainError>(TableColumn {
                    name: r.try_get("name").map_err(db_err)?,
                    column_type: r.try_get("column_type").map_err(db_err)?,
                    nullable: r.try_get("nullable").map_err(db_err)?,
                    default_expr: r.try_get("default_expr").map_err(db_err)?,
                    ordinal: r.try_get("ordinal").map_err(db_err)?,
                    comment: r.try_get("comment").map_err(db_err)?,
                    is_primary_key: r.try_get("is_primary_key").map_err(db_err)?,
                })
            })
            .collect::<Result<_, _>>()?;

        let fk_out_rows = sqlx::query(
            r#"
            SELECT
              con.conname::text       AS name,
              (
                SELECT array_agg(a.attname::text ORDER BY array_position(con.conkey, a.attnum))
                FROM pg_attribute a
                WHERE a.attrelid = con.conrelid AND a.attnum = ANY(con.conkey)
              )                       AS columns,
              tcl.relname::text       AS target_table,
              (
                SELECT array_agg(a.attname::text ORDER BY array_position(con.confkey, a.attnum))
                FROM pg_attribute a
                WHERE a.attrelid = con.confrelid AND a.attnum = ANY(con.confkey)
              )                       AS target_columns
            FROM pg_constraint con
            JOIN pg_class    scl ON scl.oid = con.conrelid
            JOIN pg_namespace sns ON sns.oid = scl.relnamespace
            JOIN pg_class    tcl ON tcl.oid = con.confrelid
            JOIN pg_namespace tns ON tns.oid = tcl.relnamespace
            WHERE con.contype = 'f'
              AND sns.nspname = $1
              AND scl.relname = $2
              AND tns.nspname = $1
            ORDER BY con.conname
            "#,
        )
        .bind(USER_SCHEMA)
        .bind(name)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        let foreign_keys_out: Vec<OutgoingForeignKey> = fk_out_rows
            .into_iter()
            .map(|r| {
                Ok::<_, DomainError>(OutgoingForeignKey {
                    name: r.try_get("name").map_err(db_err)?,
                    columns: r
                        .try_get::<Option<Vec<String>>, _>("columns")
                        .map_err(db_err)?
                        .unwrap_or_default(),
                    target_table: r.try_get("target_table").map_err(db_err)?,
                    target_columns: r
                        .try_get::<Option<Vec<String>>, _>("target_columns")
                        .map_err(db_err)?
                        .unwrap_or_default(),
                })
            })
            .collect::<Result<_, _>>()?;

        let fk_in_rows = sqlx::query(
            r#"
            SELECT
              con.conname::text       AS name,
              scl.relname::text       AS source_table,
              (
                SELECT array_agg(a.attname::text ORDER BY array_position(con.conkey, a.attnum))
                FROM pg_attribute a
                WHERE a.attrelid = con.conrelid AND a.attnum = ANY(con.conkey)
              )                       AS source_columns,
              (
                SELECT array_agg(a.attname::text ORDER BY array_position(con.confkey, a.attnum))
                FROM pg_attribute a
                WHERE a.attrelid = con.confrelid AND a.attnum = ANY(con.confkey)
              )                       AS target_columns
            FROM pg_constraint con
            JOIN pg_class    scl ON scl.oid = con.conrelid
            JOIN pg_namespace sns ON sns.oid = scl.relnamespace
            JOIN pg_class    tcl ON tcl.oid = con.confrelid
            JOIN pg_namespace tns ON tns.oid = tcl.relnamespace
            WHERE con.contype = 'f'
              AND tns.nspname = $1
              AND tcl.relname = $2
              AND sns.nspname = $1
            ORDER BY scl.relname, con.conname
            "#,
        )
        .bind(USER_SCHEMA)
        .bind(name)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        let foreign_keys_in: Vec<IncomingForeignKey> = fk_in_rows
            .into_iter()
            .map(|r| {
                Ok::<_, DomainError>(IncomingForeignKey {
                    name: r.try_get("name").map_err(db_err)?,
                    source_table: r.try_get("source_table").map_err(db_err)?,
                    source_columns: r
                        .try_get::<Option<Vec<String>>, _>("source_columns")
                        .map_err(db_err)?
                        .unwrap_or_default(),
                    target_columns: r
                        .try_get::<Option<Vec<String>>, _>("target_columns")
                        .map_err(db_err)?
                        .unwrap_or_default(),
                })
            })
            .collect::<Result<_, _>>()?;

        Ok(Some(TableSchema {
            name: name.to_owned(),
            comment,
            columns,
            foreign_keys_out,
            foreign_keys_in,
        }))
    }

    pub async fn slow_queries(&self, limit: i64) -> Result<Vec<SlowQuery>, DomainError> {
        let rows = sqlx::query(
            r#"
            SELECT
                queryid::text                    AS queryid,
                LEFT(query, 500)                 AS query,
                calls::bigint                    AS calls,
                total_exec_time::float8          AS total_exec_ms,
                mean_exec_time::float8           AS mean_exec_ms,
                rows::bigint                     AS rows
            FROM pg_stat_statements
            ORDER BY total_exec_time DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.into_iter()
            .map(|r| {
                Ok::<_, DomainError>(SlowQuery {
                    queryid: r.try_get("queryid").map_err(db_err)?,
                    query: r.try_get("query").map_err(db_err)?,
                    calls: r.try_get("calls").map_err(db_err)?,
                    total_exec_ms: r.try_get("total_exec_ms").map_err(db_err)?,
                    mean_exec_ms: r.try_get("mean_exec_ms").map_err(db_err)?,
                    rows: r.try_get("rows").map_err(db_err)?,
                })
            })
            .collect()
    }
}

/// The introspection capability, delegating to the inherent query methods.
/// Explicit `PgStatsRepository::method(self, ...)` paths avoid resolving back
/// to the trait method and recursing.
#[async_trait]
impl DbStats for PgStatsRepository {
    async fn schema_overview(&self) -> Result<RelationalSchema, DomainError> {
        PgStatsRepository::schema_overview(self).await
    }
    async fn database_size(&self) -> Result<DatabaseSize, DomainError> {
        PgStatsRepository::database_size(self).await
    }
    async fn table_stats(&self, limit: i64, offset: i64) -> Result<Vec<TableStats>, DomainError> {
        PgStatsRepository::table_stats(self, limit, offset).await
    }
    async fn index_stats(&self, limit: i64) -> Result<Vec<IndexStats>, DomainError> {
        PgStatsRepository::index_stats(self, limit).await
    }
    async fn activity(&self, limit: i64) -> Result<Vec<Activity>, DomainError> {
        PgStatsRepository::activity(self, limit).await
    }
    async fn blocking_locks(&self) -> Result<Vec<BlockingLock>, DomainError> {
        PgStatsRepository::blocking_locks(self).await
    }
    async fn table_schema(&self, name: &str) -> Result<Option<TableSchema>, DomainError> {
        PgStatsRepository::table_schema(self, name).await
    }
    async fn has_slow_query_stats(&self) -> Result<bool, DomainError> {
        PgStatsRepository::has_pg_stat_statements(self).await
    }
    async fn slow_queries(&self, limit: i64) -> Result<Vec<SlowQuery>, DomainError> {
        PgStatsRepository::slow_queries(self, limit).await
    }
    async fn optimize(&self) -> Result<(), DomainError> {
        // ANALYZE refreshes planner statistics without the heavy, blocking
        // rewrite a full VACUUM does; autovacuum handles the rest.
        sqlx::query("ANALYZE")
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
