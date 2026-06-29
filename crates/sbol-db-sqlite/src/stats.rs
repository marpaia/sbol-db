//! SQLite maintenance/observability views.
//!
//! SQLite is a single-writer embedded engine with no session/lock/per-statement
//! timing catalogs, so the session, lock, and slow-query surfaces are empty and
//! [`SqliteStats::has_slow_query_stats`] reports `false`; the lab UI gates those
//! panels on the backend's capabilities. The table/index/schema surfaces read
//! `sqlite_master` and the `PRAGMA` introspection functions. On-disk sizing uses
//! the `dbstat` virtual table when available and falls back to zero when SQLite
//! was built without it.

use async_trait::async_trait;
use sbol_db_core::DomainError;
use sbol_db_storage::{
    Activity, BlockingLock, DatabaseSize, DbStats, IncomingForeignKey, IndexStats,
    OutgoingForeignKey, RelationalColumn, RelationalSchema, RelationalTable, SlowQuery,
    TableColumn, TableSchema, TableStats,
};
use sqlx::Row;

use crate::pool::db_err;
use crate::SqlitePool;

/// The introspection capability for a SQLite backend. Cloneable; clones share
/// the pool.
#[derive(Clone)]
pub struct SqliteStats {
    pool: SqlitePool,
}

/// Quote a SQLite identifier for safe interpolation into a `PRAGMA` call, which
/// can't take a bound parameter. Embedded double-quotes are doubled.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

impl SqliteStats {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// The user tables, ordered by name: everything in `sqlite_master` that
    /// isn't an internal `sqlite_*` object or the migration bookkeeping table.
    async fn user_tables(&self) -> Result<Vec<String>, DomainError> {
        let rows = sqlx::query(
            "SELECT name FROM sqlite_master \
             WHERE type = 'table' \
               AND name NOT LIKE 'sqlite_%' \
               AND name <> '_sqlx_migrations' \
             ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter()
            .map(|r| r.try_get::<String, _>("name").map_err(db_err))
            .collect()
    }

    /// Total on-disk bytes a table or index occupies, via the `dbstat` virtual
    /// table. `dbstat` is an optional build, so any error degrades to zero.
    async fn object_bytes(&self, name: &str) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT COALESCE(SUM(pgsize), 0) FROM dbstat WHERE name = ?")
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0)
    }

    /// Whether a name exists as a user table in `sqlite_master`.
    async fn table_exists(&self, name: &str) -> Result<bool, DomainError> {
        let found: Option<String> =
            sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?")
                .bind(name)
                .fetch_optional(&self.pool)
                .await
                .map_err(db_err)?;
        Ok(found.is_some())
    }
}

#[async_trait]
impl DbStats for SqliteStats {
    async fn schema_overview(&self) -> Result<RelationalSchema, DomainError> {
        let mut tables = Vec::new();
        for name in self.user_tables().await? {
            let rows = sqlx::query(&format!("PRAGMA table_info({})", quote_ident(&name)))
                .fetch_all(&self.pool)
                .await
                .map_err(db_err)?;
            let columns = rows
                .into_iter()
                .map(|r| {
                    Ok::<_, DomainError>(RelationalColumn {
                        name: r.try_get("name").map_err(db_err)?,
                        column_type: r.try_get("type").map_err(db_err)?,
                        nullable: r.try_get::<i64, _>("notnull").map_err(db_err)? == 0,
                    })
                })
                .collect::<Result<_, _>>()?;
            tables.push(RelationalTable { name, columns });
        }
        Ok(RelationalSchema { tables })
    }

    async fn database_size(&self) -> Result<DatabaseSize, DomainError> {
        let page_count: i64 = sqlx::query_scalar("PRAGMA page_count")
            .fetch_one(&self.pool)
            .await
            .map_err(db_err)?;
        let page_size: i64 = sqlx::query_scalar("PRAGMA page_size")
            .fetch_one(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(DatabaseSize {
            database: "main".to_string(),
            total_bytes: page_count * page_size,
        })
    }

    async fn table_stats(&self, limit: i64, offset: i64) -> Result<Vec<TableStats>, DomainError> {
        let names: Vec<String> = self
            .user_tables()
            .await?
            .into_iter()
            .skip(offset.max(0) as usize)
            .take(limit.max(0) as usize)
            .collect();

        let mut stats = Vec::with_capacity(names.len());
        for name in names {
            let rows_estimate: i64 =
                sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {}", quote_ident(&name)))
                    .fetch_one(&self.pool)
                    .await
                    .map_err(db_err)?;
            let total_bytes = self.object_bytes(&name).await;
            stats.push(TableStats {
                name,
                rows_estimate,
                total_bytes,
                index_bytes: 0,
                n_live_tup: rows_estimate,
                n_dead_tup: 0,
                last_vacuum: None,
                last_autovacuum: None,
                last_analyze: None,
            });
        }
        Ok(stats)
    }

    async fn index_stats(&self, limit: i64) -> Result<Vec<IndexStats>, DomainError> {
        let rows = sqlx::query(
            "SELECT name, tbl_name FROM sqlite_master \
             WHERE type = 'index' AND name NOT LIKE 'sqlite_%' \
             ORDER BY name LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        let mut stats = Vec::with_capacity(rows.len());
        for r in rows {
            let index: String = r.try_get("name").map_err(db_err)?;
            let table: String = r.try_get("tbl_name").map_err(db_err)?;
            let bytes = self.object_bytes(&index).await;
            stats.push(IndexStats {
                table,
                index,
                idx_scan: 0,
                bytes,
            });
        }
        Ok(stats)
    }

    async fn activity(&self, _limit: i64) -> Result<Vec<Activity>, DomainError> {
        Ok(vec![])
    }

    async fn blocking_locks(&self) -> Result<Vec<BlockingLock>, DomainError> {
        Ok(vec![])
    }

    async fn table_schema(&self, name: &str) -> Result<Option<TableSchema>, DomainError> {
        if !self.table_exists(name).await? {
            return Ok(None);
        }

        let column_rows = sqlx::query(&format!("PRAGMA table_info({})", quote_ident(name)))
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        let columns = column_rows
            .into_iter()
            .map(|r| {
                Ok::<_, DomainError>(TableColumn {
                    name: r.try_get("name").map_err(db_err)?,
                    column_type: r.try_get("type").map_err(db_err)?,
                    nullable: r.try_get::<i64, _>("notnull").map_err(db_err)? == 0,
                    default_expr: r.try_get("dflt_value").map_err(db_err)?,
                    ordinal: r.try_get::<i64, _>("cid").map_err(db_err)? as i32 + 1,
                    comment: None,
                    is_primary_key: r.try_get::<i64, _>("pk").map_err(db_err)? != 0,
                })
            })
            .collect::<Result<_, _>>()?;

        let foreign_keys_out = self.foreign_keys_out(name).await?;
        let foreign_keys_in = self.foreign_keys_in(name).await?;

        Ok(Some(TableSchema {
            name: name.to_string(),
            comment: None,
            columns,
            foreign_keys_out,
            foreign_keys_in,
        }))
    }

    async fn has_slow_query_stats(&self) -> Result<bool, DomainError> {
        Ok(false)
    }

    async fn slow_queries(&self, _limit: i64) -> Result<Vec<SlowQuery>, DomainError> {
        Ok(vec![])
    }

    async fn optimize(&self) -> Result<(), DomainError> {
        sqlx::query("VACUUM;")
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        sqlx::query("ANALYZE;")
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
}

impl SqliteStats {
    /// Outgoing foreign keys for `name`, grouped by `PRAGMA foreign_key_list`'s
    /// `id` column (one constraint can span several columns).
    async fn foreign_keys_out(&self, name: &str) -> Result<Vec<OutgoingForeignKey>, DomainError> {
        let rows = sqlx::query(&format!("PRAGMA foreign_key_list({})", quote_ident(name)))
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;

        // Preserve first-seen order of each constraint id.
        let mut order: Vec<i64> = Vec::new();
        let mut by_id: std::collections::HashMap<i64, OutgoingForeignKey> =
            std::collections::HashMap::new();
        for r in rows {
            let id: i64 = r.try_get("id").map_err(db_err)?;
            let from: String = r.try_get("from").map_err(db_err)?;
            let to: String = r.try_get("to").map_err(db_err)?;
            let target_table: String = r.try_get("table").map_err(db_err)?;
            let entry = by_id.entry(id).or_insert_with(|| {
                order.push(id);
                OutgoingForeignKey {
                    name: format!("fk_{id}"),
                    columns: Vec::new(),
                    target_table,
                    target_columns: Vec::new(),
                }
            });
            entry.columns.push(from);
            entry.target_columns.push(to);
        }
        Ok(order
            .into_iter()
            .filter_map(|id| by_id.remove(&id))
            .collect())
    }

    /// Incoming foreign keys: scan every other user table's
    /// `PRAGMA foreign_key_list` for references back to `name`.
    async fn foreign_keys_in(&self, name: &str) -> Result<Vec<IncomingForeignKey>, DomainError> {
        let mut incoming = Vec::new();
        for source in self.user_tables().await? {
            if source == name {
                continue;
            }
            let rows = sqlx::query(&format!(
                "PRAGMA foreign_key_list({})",
                quote_ident(&source)
            ))
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;

            let mut order: Vec<i64> = Vec::new();
            let mut by_id: std::collections::HashMap<i64, IncomingForeignKey> =
                std::collections::HashMap::new();
            for r in rows {
                let target_table: String = r.try_get("table").map_err(db_err)?;
                if target_table != name {
                    continue;
                }
                let id: i64 = r.try_get("id").map_err(db_err)?;
                let from: String = r.try_get("from").map_err(db_err)?;
                let to: String = r.try_get("to").map_err(db_err)?;
                let entry = by_id.entry(id).or_insert_with(|| {
                    order.push(id);
                    IncomingForeignKey {
                        name: format!("fk_{id}"),
                        source_table: source.clone(),
                        source_columns: Vec::new(),
                        target_columns: Vec::new(),
                    }
                });
                entry.source_columns.push(from);
                entry.target_columns.push(to);
            }
            for id in order {
                if let Some(fk) = by_id.remove(&id) {
                    incoming.push(fk);
                }
            }
        }
        Ok(incoming)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{connect, run_migrations};

    async fn stats() -> SqliteStats {
        let pool = connect("sqlite::memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        SqliteStats::new(pool)
    }

    #[tokio::test]
    async fn schema_overview_lists_known_tables() {
        let stats = stats().await;
        let schema = stats.schema_overview().await.unwrap();
        let names: Vec<&str> = schema.tables.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"sbol_graphs"));
        assert!(names.contains(&"sbol_triples"));
        assert!(names.contains(&"sbol_objects"));
        assert!(!names.contains(&"_sqlx_migrations"));
        let graphs = schema
            .tables
            .iter()
            .find(|t| t.name == "sbol_graphs")
            .unwrap();
        assert!(!graphs.columns.is_empty());
    }

    #[tokio::test]
    async fn database_size_is_positive() {
        let stats = stats().await;
        let size = stats.database_size().await.unwrap();
        assert_eq!(size.database, "main");
        assert!(size.total_bytes > 0);
    }

    #[tokio::test]
    async fn table_schema_returns_columns() {
        let stats = stats().await;
        let schema = stats.table_schema("sbol_triples").await.unwrap().unwrap();
        assert_eq!(schema.name, "sbol_triples");
        assert!(!schema.columns.is_empty());
        let unknown = stats.table_schema("does_not_exist").await.unwrap();
        assert!(unknown.is_none());
    }

    #[tokio::test]
    async fn table_stats_and_indexes() {
        let stats = stats().await;
        let tables = stats.table_stats(100, 0).await.unwrap();
        assert!(tables.iter().any(|t| t.name == "sbol_triples"));
        // sbol_triples references sbol_graphs, so it has an outgoing FK.
        let schema = stats.table_schema("sbol_triples").await.unwrap().unwrap();
        assert!(!schema.foreign_keys_out.is_empty());
        let _ = stats.index_stats(50).await.unwrap();
    }

    #[tokio::test]
    async fn empty_observability_surfaces() {
        let stats = stats().await;
        assert!(stats.activity(10).await.unwrap().is_empty());
        assert!(stats.blocking_locks().await.unwrap().is_empty());
        assert!(!stats.has_slow_query_stats().await.unwrap());
        assert!(stats.slow_queries(10).await.unwrap().is_empty());
    }
}
