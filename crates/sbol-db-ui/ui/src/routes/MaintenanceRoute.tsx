/**
 * Backend maintenance dashboard. The shape depends on the active
 * storage backend:
 *
 *  - Relational (Postgres, SQLite): read-only views over the catalog —
 *    database/table/index size, live activity, blocking locks, and
 *    slow queries — plus an Optimize action (SQLite VACUUM/ANALYZE,
 *    Postgres ANALYZE). Activity, locks, and slow queries are gated on
 *    the backend's capabilities.
 *  - LSM (RocksDB): store size, key estimates, column families, and
 *    per-level file counts, plus a Compact action.
 *
 * Polled every 15 s. Backed by `/lab/api/observability/maintenance/*`.
 * The list of tables is clickable — each row drills into
 * `/schema/tables/:name` for a per-table view.
 */

import { useMemo } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Info, Loader2, Wrench } from "lucide-react";
import { useNavigate } from "react-router-dom";

import { BackendUnavailable } from "@/components/lab/BackendUnavailable";
import { DataTable, type DataTableColumn } from "@/components/lab/DataTable";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { KpiTile } from "@/components/observability/KpiTile";
import { useBackendInfo } from "@/hooks/useBackendInfo";
import {
  useLsmOverview,
  useMaintenanceActivity,
  useMaintenanceDatabase,
  useMaintenanceIndexes,
  useMaintenanceLocks,
  useMaintenanceSlowQueries,
  useMaintenanceTables,
} from "@/hooks/useObservability";
import {
  postCompact,
  postOptimize,
  type BlockingLock,
  type Capabilities,
  type IndexStats,
  type LsmColumnFamily,
  type LsmLevel,
  type PgActivity,
  type SlowQuery,
  type TableStats,
} from "@/lib/api";
import {
  cn,
  describeError,
  formatBytes,
  formatMs,
  formatRelative,
} from "@/lib/utils";

export default function MaintenanceRoute() {
  const { data: info } = useBackendInfo();
  const maintenance = info?.capabilities.maintenance ?? null;

  if (info && maintenance === null) {
    return <BackendUnavailable feature="Maintenance" />;
  }

  if (maintenance === "lsm") {
    return <LsmMaintenance />;
  }

  return <RelationalMaintenance capabilities={info?.capabilities} />;
}

// ---------- Relational backends (Postgres, SQLite) ----------

function RelationalMaintenance({
  capabilities,
}: {
  capabilities?: Capabilities;
}) {
  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-8 px-8 py-10">
        <header className="flex flex-wrap items-start justify-between gap-4">
          <div className="min-w-0">
            <h1 className="text-2xl font-semibold tracking-tight">
              Maintenance
            </h1>
            <p className="mt-2 text-sm text-muted-foreground">
              Storage, table, and index sizes and engine health. Sampled every
              15 seconds. Click a table to drill in.
            </p>
          </div>
          <OptimizeButton />
        </header>

        <DatabaseOverview />
        <TablesSection />
        <IndexesSection />
        {capabilities?.activity_and_locks && (
          <>
            <ActivitySection />
            <LocksSection />
          </>
        )}
        {capabilities?.slow_query_stats && <SlowQueriesSection />}
      </div>
    </div>
  );
}

function OptimizeButton() {
  const qc = useQueryClient();
  const mutation = useMutation({
    mutationFn: () => postOptimize(),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["lab", "obs", "maintenance"] });
    },
  });

  return (
    <button
      type="button"
      onClick={() => mutation.mutate()}
      disabled={mutation.isPending}
      className="inline-flex items-center gap-1.5 rounded-md border bg-background px-3 py-1.5 text-sm font-medium text-foreground transition-colors hover:bg-accent disabled:opacity-50"
      title="Reclaim space and refresh planner statistics"
    >
      {mutation.isPending ? (
        <Loader2 size={12} className="animate-spin" />
      ) : (
        <Wrench size={12} />
      )}
      Optimize
    </button>
  );
}

function DatabaseOverview() {
  const { data, isLoading, error } = useMaintenanceDatabase();

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
          <div
            key={i}
            className="h-24 animate-pulse rounded-lg border bg-card"
          />
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
  const { data, isLoading, error } = useMaintenanceTables(200, 0);
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
          title="Couldn't read table statistics"
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
  const { data, isLoading, error } = useMaintenanceIndexes(50);
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
          <span className="truncate font-mono text-foreground" title={i.index}>
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
          title="Couldn't read index statistics"
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
  const { data, isLoading, error } = useMaintenanceActivity(50);

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
            {a.wait_event ? `${a.wait_event_type ?? ""}:${a.wait_event}` : "—"}
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
    <Panel title="Live activity" subtitle="excl. our own backend">
      {error ? (
        <ErrorBanner
          title="Couldn't read live activity"
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
  const { data, error } = useMaintenanceLocks();

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
    <Panel title="Blocking locks" subtitle="blocker → blocked">
      {error ? (
        <ErrorBanner
          title="Couldn't read blocking locks"
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
          rowKey={(r) =>
            `${r.blocker_pid}-${r.blocked_pid}-${r.locktype ?? ""}-${r.mode ?? ""}`
          }
        />
      )}
    </Panel>
  );
}

function SlowQueriesSection() {
  const { data, isLoading, error } = useMaintenanceSlowQueries(20);

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
    <Panel title="Slow queries" subtitle="top 20 by total time">
      {error ? (
        <ErrorBanner
          title="Couldn't read slow queries"
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

// ---------- LSM backends (RocksDB) ----------

function LsmMaintenance() {
  const { data, isLoading, error } = useLsmOverview();

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-8 px-8 py-10">
        <header className="flex flex-wrap items-start justify-between gap-4">
          <div className="min-w-0">
            <h1 className="text-2xl font-semibold tracking-tight">
              Maintenance
            </h1>
            <p className="mt-2 text-sm text-muted-foreground">
              Store size, key estimates, column families, and per-level files.
              Sampled every 15 seconds.
            </p>
          </div>
          <CompactButton />
        </header>

        {error ? (
          <ErrorBanner
            title="Couldn't read store statistics"
            body={describeError(error)}
          />
        ) : isLoading && !data ? (
          <LsmSkeleton />
        ) : !data ? null : (
          <>
            <section className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
              <KpiTile
                label="total size"
                value={formatBytes(data.total_bytes)}
              />
              <KpiTile
                label="estimated keys"
                value={data.estimated_keys.toLocaleString()}
              />
              <KpiTile
                label="pending compaction"
                value={formatBytes(data.pending_compaction_bytes)}
                tone={data.pending_compaction_bytes > 0 ? "warn" : "default"}
              />
              <KpiTile
                label="running compactions"
                value={data.running_compactions}
                tone={data.running_compactions > 0 ? "warn" : "default"}
              />
            </section>

            <ColumnFamiliesSection families={data.column_families} />
            <LevelsSection levels={data.levels} />
          </>
        )}
      </div>
    </div>
  );
}

function CompactButton() {
  const qc = useQueryClient();
  const mutation = useMutation({
    mutationFn: () => postCompact(),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["lab", "obs", "maintenance"] });
    },
  });

  return (
    <button
      type="button"
      onClick={() => mutation.mutate()}
      disabled={mutation.isPending}
      className="inline-flex items-center gap-1.5 rounded-md border bg-background px-3 py-1.5 text-sm font-medium text-foreground transition-colors hover:bg-accent disabled:opacity-50"
      title="Compact the store to reclaim space and merge files"
    >
      {mutation.isPending ? (
        <Loader2 size={12} className="animate-spin" />
      ) : (
        <Wrench size={12} />
      )}
      Compact
    </button>
  );
}

function ColumnFamiliesSection({ families }: { families: LsmColumnFamily[] }) {
  const columns = useMemo<DataTableColumn<LsmColumnFamily>[]>(
    () => [
      {
        id: "name",
        header: "column family",
        width: 280,
        sortValue: (f) => f.name,
        filterValue: (f) => f.name,
        cell: (f) => (
          <span className="truncate font-mono text-foreground" title={f.name}>
            {f.name}
          </span>
        ),
      },
      {
        id: "files",
        header: "files",
        width: 100,
        align: "right",
        sortValue: (f) => f.num_files,
        cell: (f) => (
          <span className="tabular-nums text-muted-foreground">
            {f.num_files.toLocaleString()}
          </span>
        ),
      },
      {
        id: "size",
        header: "size",
        width: 120,
        align: "right",
        sortValue: (f) => f.size_bytes,
        cell: (f) => (
          <span className="tabular-nums text-foreground">
            {formatBytes(f.size_bytes)}
          </span>
        ),
      },
      {
        id: "keys",
        header: "est. keys",
        width: 120,
        align: "right",
        sortValue: (f) => f.estimated_keys,
        cell: (f) => (
          <span className="tabular-nums text-muted-foreground">
            {f.estimated_keys.toLocaleString()}
          </span>
        ),
      },
    ],
    []
  );

  return (
    <Panel title="Column families" subtitle={`${families.length} total`}>
      {families.length === 0 ? (
        <Empty>No column families reported.</Empty>
      ) : (
        <DataTable
          columns={columns}
          rows={families}
          rowKey={(f) => f.name}
          filterable
          defaultSort={{ id: "size", direction: "desc" }}
        />
      )}
    </Panel>
  );
}

function LevelsSection({ levels }: { levels: LsmLevel[] }) {
  const columns = useMemo<DataTableColumn<LsmLevel>[]>(
    () => [
      {
        id: "level",
        header: "level",
        width: 100,
        align: "right",
        sortValue: (l) => l.level,
        cell: (l) => (
          <span className="tabular-nums text-foreground">L{l.level}</span>
        ),
      },
      {
        id: "files",
        header: "files",
        width: 120,
        align: "right",
        sortValue: (l) => l.num_files,
        cell: (l) => (
          <span className="tabular-nums text-muted-foreground">
            {l.num_files.toLocaleString()}
          </span>
        ),
      },
      {
        id: "size",
        header: "size",
        width: 120,
        align: "right",
        sortValue: (l) => l.size_bytes,
        cell: (l) => (
          <span className="tabular-nums text-foreground">
            {formatBytes(l.size_bytes)}
          </span>
        ),
      },
    ],
    []
  );

  return (
    <Panel title="Levels" subtitle={`${levels.length} levels`}>
      {levels.length === 0 ? (
        <Empty>No level data reported.</Empty>
      ) : (
        <DataTable
          columns={columns}
          rows={levels}
          rowKey={(l) => String(l.level)}
          defaultSort={{ id: "level", direction: "asc" }}
        />
      )}
    </Panel>
  );
}

function LsmSkeleton() {
  return (
    <div className="space-y-4">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
        {Array.from({ length: 4 }).map((_, i) => (
          <div
            key={i}
            className="h-24 animate-pulse rounded-lg border bg-card"
          />
        ))}
      </div>
      <div className="h-48 animate-pulse rounded-lg border bg-card" />
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
  return (
    <div className="px-4 py-4 text-sm text-muted-foreground">{children}</div>
  );
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
