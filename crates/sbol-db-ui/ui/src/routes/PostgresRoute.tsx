/**
 * Postgres maintenance dashboard.
 *
 * Read-only views over the catalog: database/table/index size,
 * autovacuum lag, live activity from `pg_stat_activity`, blocker →
 * blocked pairs from `pg_locks`, and (when installed) top slow queries
 * from `pg_stat_statements`. Backed by `/lab/api/observability/postgres/*`
 * and polled every 15 s. No destructive actions in v1.
 *
 * The table list is clickable — each row drills into
 * `/schema/tables/:name` for a per-table view
 * matching the Explore / Schema route's drill-down pattern.
 */

import { useMemo } from "react";
import { Info } from "lucide-react";
import { useNavigate } from "react-router-dom";

import {
  DataTable,
  type DataTableColumn,
} from "@/components/lab/DataTable";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { KpiTile } from "@/components/observability/KpiTile";
import {
  usePgActivity,
  usePgDatabase,
  usePgIndexes,
  usePgLocks,
  usePgSlowQueries,
  usePgTables,
} from "@/hooks/useObservability";
import type {
  BlockingLock,
  IndexStats,
  PgActivity,
  SlowQuery,
  TableStats,
} from "@/lib/api";
import {
  cn,
  describeError,
  formatBytes,
  formatMs,
  formatRelative,
} from "@/lib/utils";

export default function PostgresRoute() {
  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-8 px-8 py-10">
        <header>
          <h1 className="text-2xl font-semibold tracking-tight">
            Postgres Maintenance
          </h1>
          <p className="mt-2 text-sm text-muted-foreground">
            Sizes, autovacuum, live activity, blocking locks, and slow
            queries. Sampled every 15 seconds. Click a table to drill in.
            Read-only.
          </p>
        </header>

        <DatabaseOverview />
        <TablesSection />
        <IndexesSection />
        <ActivitySection />
        <LocksSection />
        <SlowQueriesSection />
      </div>
    </div>
  );
}

function DatabaseOverview() {
  const { data, isLoading, error } = usePgDatabase();

  if (error) {
    return (
      <ErrorBanner
        title="Couldn't read database size"
        body={describeError(error)}
      />
    );
  }
  if (!data) {
    return (
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
        {Array.from({ length: 3 }).map((_, i) => (
          <div key={i} className="h-24 animate-pulse rounded-lg border bg-card" />
        ))}
      </div>
    );
  }

  return (
    <section>
      <KpiTile
        label="database size"
        value={formatBytes(data.total_bytes)}
        hint={data.database}
        loading={isLoading && !data}
      />
    </section>
  );
}

function TablesSection() {
  const { data, isLoading, error } = usePgTables(200, 0);
  const navigate = useNavigate();

  const columns = useMemo<DataTableColumn<TableStats>[]>(
    () => [
      {
        id: "name",
        header: "table",
        width: 280,
        sortValue: (t) => t.name,
        filterValue: (t) => t.name,
        cell: (t) => (
          <span className="truncate font-mono text-foreground" title={t.name}>
            {t.name}
          </span>
        ),
      },
      {
        id: "rows",
        header: "rows (est)",
        width: 110,
        align: "right",
        sortValue: (t) => t.rows_estimate,
        cell: (t) => (
          <span className="tabular-nums text-muted-foreground">
            {t.rows_estimate.toLocaleString()}
          </span>
        ),
      },
      {
        id: "size",
        header: "size",
        width: 100,
        align: "right",
        sortValue: (t) => t.total_bytes,
        cell: (t) => (
          <span className="tabular-nums text-foreground">
            {formatBytes(t.total_bytes)}
          </span>
        ),
      },
      {
        id: "indexes",
        header: "indexes",
        width: 100,
        align: "right",
        sortValue: (t) => t.index_bytes,
        cell: (t) => (
          <span className="tabular-nums text-muted-foreground">
            {formatBytes(t.index_bytes)}
          </span>
        ),
      },
      {
        id: "dead",
        header: "dead %",
        width: 90,
        align: "right",
        sortValue: (t) => deadPctOf(t),
        cell: (t) => {
          const pct = deadPctOf(t);
          return (
            <span
              className={cn(
                "tabular-nums",
                pct >= 50
                  ? "text-destructive"
                  : pct >= 20
                    ? "text-amber-500"
                    : "text-muted-foreground"
              )}
            >
              {pct.toFixed(1)}%
            </span>
          );
        },
      },
      {
        id: "vacuum",
        header: "last vacuum",
        width: 130,
        sortValue: (t) =>
          new Date(t.last_autovacuum ?? t.last_vacuum ?? 0).getTime() || 0,
        cell: (t) => (
          <span className="text-muted-foreground">
            {formatRelative(t.last_autovacuum ?? t.last_vacuum)}
          </span>
        ),
      },
    ],
    []
  );

  return (
    <Panel
      title="Tables"
      subtitle="top by total relation size, click to drill in"
    >
      {error ? (
        <ErrorBanner
          title="Couldn't read pg_stat_user_tables"
          body={describeError(error)}
        />
      ) : isLoading && !data ? (
        <RowSkeleton rows={6} />
      ) : !data || data.length === 0 ? (
        <Empty>No user tables yet.</Empty>
      ) : (
        <DataTable
          columns={columns}
          rows={data}
          rowKey={(t) => t.name}
          filterable
          defaultSort={{ id: "size", direction: "desc" }}
          onRowClick={(t) =>
            navigate(`/schema/tables/${encodeURIComponent(t.name)}`)
          }
        />
      )}
    </Panel>
  );
}

function deadPctOf(t: TableStats): number {
  const total = t.n_live_tup + t.n_dead_tup;
  return total > 0 ? (t.n_dead_tup / total) * 100 : 0;
}

function IndexesSection() {
  const { data, isLoading, error } = usePgIndexes(50);
  const unused = useMemo(
    () => (data ?? []).filter((i) => i.idx_scan === 0).length,
    [data]
  );

  const columns = useMemo<DataTableColumn<IndexStats>[]>(
    () => [
      {
        id: "index",
        header: "index",
        width: 320,
        sortValue: (i) => i.index,
        filterValue: (i) => `${i.index} ${i.table}`,
        cell: (i) => (
          <span
            className="truncate font-mono text-foreground"
            title={i.index}
          >
            {i.index}
          </span>
        ),
      },
      {
        id: "table",
        header: "table",
        width: 220,
        sortValue: (i) => i.table,
        cell: (i) => (
          <span
            className="truncate font-mono text-muted-foreground"
            title={i.table}
          >
            {i.table}
          </span>
        ),
      },
      {
        id: "scans",
        header: "scans",
        width: 100,
        align: "right",
        sortValue: (i) => i.idx_scan,
        cell: (i) => (
          <span
            className={cn(
              "tabular-nums",
              i.idx_scan === 0 ? "text-amber-500" : "text-muted-foreground"
            )}
          >
            {i.idx_scan.toLocaleString()}
          </span>
        ),
      },
      {
        id: "size",
        header: "size",
        width: 100,
        align: "right",
        sortValue: (i) => i.bytes,
        cell: (i) => (
          <span className="tabular-nums text-foreground">
            {formatBytes(i.bytes)}
          </span>
        ),
      },
    ],
    []
  );

  return (
    <Panel
      title="Indexes"
      subtitle={`${data?.length ?? 0} listed · ${unused} with idx_scan = 0`}
    >
      {error ? (
        <ErrorBanner
          title="Couldn't read pg_stat_user_indexes"
          body={describeError(error)}
        />
      ) : isLoading && !data ? (
        <RowSkeleton rows={6} />
      ) : !data || data.length === 0 ? (
        <Empty>No user indexes yet.</Empty>
      ) : (
        <DataTable
          columns={columns}
          rows={data}
          rowKey={(i) => i.index}
          filterable
          defaultSort={{ id: "size", direction: "desc" }}
        />
      )}
    </Panel>
  );
}

function ActivitySection() {
  const { data, isLoading, error } = usePgActivity(50);

  const columns = useMemo<DataTableColumn<PgActivity>[]>(
    () => [
      {
        id: "pid",
        header: "pid",
        width: 80,
        align: "right",
        sortValue: (a) => a.pid,
        cell: (a) => (
          <span className="tabular-nums text-muted-foreground">{a.pid}</span>
        ),
      },
      {
        id: "state",
        header: "state",
        width: 120,
        sortValue: (a) => a.state ?? "",
        cell: (a) => (
          <span className="truncate text-foreground">{a.state ?? "—"}</span>
        ),
      },
      {
        id: "wait",
        header: "wait",
        width: 180,
        sortValue: (a) => a.wait_event ?? "",
        cell: (a) => (
          <span className="truncate text-muted-foreground">
            {a.wait_event
              ? `${a.wait_event_type ?? ""}:${a.wait_event}`
              : "—"}
          </span>
        ),
      },
      {
        id: "duration",
        header: "duration",
        width: 100,
        align: "right",
        sortValue: (a) => a.duration_secs ?? -1,
        cell: (a) => (
          <span className="tabular-nums text-foreground">
            {a.duration_secs !== null ? formatMs(a.duration_secs * 1000) : "—"}
          </span>
        ),
      },
      {
        id: "query",
        header: "query",
        width: 480,
        filterValue: (a) => a.query ?? undefined,
        cell: (a) => (
          <span
            className="truncate font-mono text-muted-foreground"
            title={a.query ?? undefined}
          >
            {(a.query ?? "").replace(/\s+/g, " ")}
          </span>
        ),
      },
    ],
    []
  );

  return (
    <Panel title="Live activity" subtitle="pg_stat_activity · excl. our own backend">
      {error ? (
        <ErrorBanner
          title="Couldn't read pg_stat_activity"
          body={describeError(error)}
        />
      ) : isLoading && !data ? (
        <RowSkeleton rows={4} />
      ) : !data || data.length === 0 ? (
        <Empty>No other client backends right now.</Empty>
      ) : (
        <DataTable
          columns={columns}
          rows={data}
          rowKey={(a) => String(a.pid)}
          filterable
          defaultSort={{ id: "duration", direction: "desc" }}
        />
      )}
    </Panel>
  );
}

function LocksSection() {
  const { data, error } = usePgLocks();

  const columns = useMemo<DataTableColumn<BlockingLock>[]>(
    () => [
      {
        id: "blocker",
        header: "blocker pid",
        width: 100,
        align: "right",
        sortValue: (r) => r.blocker_pid,
        cell: (r) => (
          <span className="tabular-nums text-foreground">{r.blocker_pid}</span>
        ),
      },
      {
        id: "blocked",
        header: "blocked pid",
        width: 100,
        align: "right",
        sortValue: (r) => r.blocked_pid,
        cell: (r) => (
          <span className="tabular-nums text-foreground">{r.blocked_pid}</span>
        ),
      },
      {
        id: "mode",
        header: "mode",
        width: 140,
        sortValue: (r) => r.mode ?? "",
        cell: (r) => (
          <span className="truncate text-muted-foreground">
            {r.mode ?? "—"}
          </span>
        ),
      },
      {
        id: "blocker_query",
        header: "blocker query",
        width: 320,
        cell: (r) => (
          <span
            className="truncate font-mono text-foreground"
            title={r.blocker_query ?? undefined}
          >
            {(r.blocker_query ?? "—").replace(/\s+/g, " ")}
          </span>
        ),
      },
      {
        id: "blocked_query",
        header: "blocked query",
        width: 320,
        cell: (r) => (
          <span
            className="truncate font-mono text-muted-foreground"
            title={r.blocked_query ?? undefined}
          >
            ↳ {(r.blocked_query ?? "—").replace(/\s+/g, " ")}
          </span>
        ),
      },
    ],
    []
  );

  return (
    <Panel title="Blocking locks" subtitle="pg_locks · blocker → blocked">
      {error ? (
        <ErrorBanner
          title="Couldn't read pg_locks"
          body={describeError(error)}
        />
      ) : !data ? (
        <RowSkeleton rows={2} />
      ) : data.length === 0 ? (
        <Empty>No blocking locks. 🎉</Empty>
      ) : (
        <DataTable
          columns={columns}
          rows={data}
          rowKey={(r) => `${r.blocker_pid}-${r.blocked_pid}-${r.locktype ?? ""}-${r.mode ?? ""}`}
        />
      )}
    </Panel>
  );
}

function SlowQueriesSection() {
  const { data, isLoading, error } = usePgSlowQueries(20);

  const columns = useMemo<DataTableColumn<SlowQuery>[]>(
    () => [
      {
        id: "query",
        header: "query",
        width: 480,
        filterValue: (r) => r.query ?? undefined,
        cell: (r) => (
          <span
            className="truncate font-mono text-foreground"
            title={r.query ?? undefined}
          >
            {(r.query ?? "").replace(/\s+/g, " ")}
          </span>
        ),
      },
      {
        id: "calls",
        header: "calls",
        width: 100,
        align: "right",
        sortValue: (r) => r.calls,
        cell: (r) => (
          <span className="tabular-nums text-muted-foreground">
            {r.calls.toLocaleString()}
          </span>
        ),
      },
      {
        id: "total",
        header: "total",
        width: 110,
        align: "right",
        sortValue: (r) => r.total_exec_ms,
        cell: (r) => (
          <span className="tabular-nums text-foreground">
            {formatMs(r.total_exec_ms)}
          </span>
        ),
      },
      {
        id: "mean",
        header: "mean",
        width: 110,
        align: "right",
        sortValue: (r) => r.mean_exec_ms,
        cell: (r) => (
          <span className="tabular-nums text-muted-foreground">
            {formatMs(r.mean_exec_ms)}
          </span>
        ),
      },
      {
        id: "rows",
        header: "rows",
        width: 100,
        align: "right",
        sortValue: (r) => r.rows,
        cell: (r) => (
          <span className="tabular-nums text-muted-foreground">
            {r.rows.toLocaleString()}
          </span>
        ),
      },
    ],
    []
  );

  return (
    <Panel title="Slow queries" subtitle="pg_stat_statements · top 20 by total time">
      {error ? (
        <ErrorBanner
          title="Couldn't read pg_stat_statements"
          body={describeError(error)}
        />
      ) : isLoading && !data ? (
        <RowSkeleton rows={3} />
      ) : !data ? null : data.status === "not_installed" ? (
        <NotInstalled hint={data.setup_hint} />
      ) : data.rows.length === 0 ? (
        <Empty>No queries recorded yet.</Empty>
      ) : (
        <DataTable
          columns={columns}
          rows={data.rows}
          rowKey={(r) => r.queryid}
          filterable
          defaultSort={{ id: "total", direction: "desc" }}
        />
      )}
    </Panel>
  );
}

function NotInstalled({ hint }: { hint: string }) {
  return (
    <div className="flex items-start gap-3 px-4 py-4">
      <Info size={14} className="mt-0.5 shrink-0 text-muted-foreground" />
      <div className="min-w-0 space-y-1">
        <div className="text-sm font-medium text-foreground">
          pg_stat_statements isn't installed
        </div>
        <pre className="whitespace-pre-wrap font-mono text-[11px] text-muted-foreground">
          {hint}
        </pre>
      </div>
    </div>
  );
}

// ---------- Layout helpers ----------

function Panel({
  title,
  subtitle,
  children,
}: {
  title: string;
  subtitle?: string;
  children: React.ReactNode;
}) {
  return (
    <section className="overflow-hidden rounded-lg border bg-card">
      <header className="flex items-center gap-2 border-b px-4 py-2">
        <h2 className="text-sm font-medium">{title}</h2>
        {subtitle && (
          <span className="text-xs text-muted-foreground">{subtitle}</span>
        )}
      </header>
      <div>{children}</div>
    </section>
  );
}

function Empty({ children }: { children: React.ReactNode }) {
  return <div className="px-4 py-4 text-sm text-muted-foreground">{children}</div>;
}

function RowSkeleton({ rows }: { rows: number }) {
  return (
    <div className="divide-y">
      {Array.from({ length: rows }).map((_, i) => (
        <div key={i} className="flex items-center gap-3 px-4 py-2">
          <div className="h-3 w-24 animate-pulse rounded bg-muted" />
          <div className="h-3 flex-1 animate-pulse rounded bg-muted" />
        </div>
      ))}
    </div>
  );
}
