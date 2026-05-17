/**
 * Per-table drill-down. Reached by clicking a row on the Schema page
 * (or the Postgres maintenance page) at `/schema/tables/:name`.
 *
 * Shows table-level metadata: size, dead percentage, last vacuum,
 * column definitions, foreign-key references, and indexes. Includes a
 * "Query" launcher that drops a starter `SELECT * FROM <table>` into
 * the SQL editor.
 *
 * Postgres schemas are not exposed as a domain concept — every table
 * here is read from `public` on the server. The URL is name-only.
 */

import { useMemo } from "react";
import { ChevronLeft, ChevronRight, HardDrive, Key, Play } from "lucide-react";
import { Link, useNavigate, useParams } from "react-router-dom";

import { DataTable, type DataTableColumn } from "@/components/lab/DataTable";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { KpiTile } from "@/components/observability/KpiTile";
import {
  usePgIndexes,
  usePgTables,
  usePgTableSchema,
} from "@/hooks/useObservability";
import type {
  IncomingForeignKey,
  IndexStats,
  OutgoingForeignKey,
  TableColumn,
  TableSchema,
  TableStats,
} from "@/lib/api";
import { useLabStore } from "@/lib/store";
import { cn, describeError, formatBytes, formatRelative } from "@/lib/utils";

export default function PgTableDetailRoute() {
  const params = useParams<{ name: string }>();
  const name = decodeURIComponent(params.name ?? "");
  const navigate = useNavigate();
  const setBuffer = useLabStore((s) => s.setBuffer);

  const tablesQuery = usePgTables(200, 0);
  const indexesQuery = usePgIndexes(200);
  const schemaQuery = usePgTableSchema(name);

  const table = useMemo<TableStats | undefined>(
    () => tablesQuery.data?.find((t) => t.name === name),
    [tablesQuery.data, name]
  );

  const indexes = useMemo<IndexStats[]>(
    () => (indexesQuery.data ?? []).filter((i) => i.table === name),
    [indexesQuery.data, name]
  );

  const launchQuery = () => {
    const qualified = needsQuoting(name) ? `"${name}"` : name;
    const template = `SELECT *\nFROM ${qualified}\nLIMIT 100;\n`;
    setBuffer("sql", template);
    navigate("/sql");
  };

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-6 px-8 py-10">
        <Link
          to="/schema"
          className="inline-flex items-center gap-1 text-xs text-muted-foreground transition-colors hover:text-foreground"
        >
          <ChevronLeft size={12} />
          Schema browser
        </Link>

        <header className="flex flex-wrap items-start justify-between gap-4">
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <HardDrive
                size={16}
                className="shrink-0 text-muted-foreground/70"
              />
              <h1 className="truncate font-mono text-xl font-semibold tracking-tight">
                {name}
              </h1>
            </div>
          </div>
          <button
            type="button"
            onClick={launchQuery}
            disabled={!table}
            className="inline-flex items-center gap-1.5 rounded-md border bg-background px-3 py-1.5 text-sm font-medium text-foreground transition-colors hover:bg-accent disabled:opacity-50"
            title={`Insert SELECT * FROM ${name} into the SQL editor`}
          >
            <Play size={12} />
            Query
          </button>
        </header>

        {tablesQuery.error ? (
          <ErrorBanner
            title="Couldn't read pg_stat_user_tables"
            body={describeError(tablesQuery.error)}
          />
        ) : tablesQuery.isLoading && !table ? (
          <KpiSkeleton />
        ) : !table ? (
          <NotFound name={name} />
        ) : (
          <TableKpis table={table} />
        )}

        <SchemaSection
          loading={schemaQuery.isLoading && !schemaQuery.data}
          error={schemaQuery.error}
          data={schemaQuery.data ?? null}
        />

        <IndexesPanel
          loading={indexesQuery.isLoading && !indexesQuery.data}
          error={indexesQuery.error}
          rows={indexes}
        />
      </div>
    </div>
  );
}

function SchemaSection({
  loading,
  error,
  data,
}: {
  loading: boolean;
  error: unknown;
  data: TableSchema | null;
}) {
  if (error) {
    return (
      <ErrorBanner
        title="Couldn't load table schema"
        body={describeError(error)}
      />
    );
  }
  if (loading) {
    return (
      <section className="overflow-hidden rounded-lg border bg-card">
        <header className="flex items-center gap-2 border-b px-4 py-2">
          <h2 className="text-sm font-medium">Columns</h2>
        </header>
        <div className="px-4 py-4 text-sm text-muted-foreground">
          Loading schema…
        </div>
      </section>
    );
  }
  if (!data) return null;

  return (
    <>
      {data.comment && (
        <section className="rounded-lg border bg-card px-4 py-3 text-sm text-muted-foreground">
          {data.comment}
        </section>
      )}
      <ColumnsPanel columns={data.columns} />
      {data.foreign_keys_out.length > 0 && (
        <ForeignKeysOutPanel rows={data.foreign_keys_out} />
      )}
      {data.foreign_keys_in.length > 0 && (
        <ForeignKeysInPanel rows={data.foreign_keys_in} />
      )}
    </>
  );
}

function ColumnsPanel({ columns }: { columns: TableColumn[] }) {
  const pkCount = columns.filter((c) => c.is_primary_key).length;
  const dataColumns = useMemo<DataTableColumn<TableColumn>[]>(
    () => [
      {
        id: "name",
        header: "column",
        width: 240,
        sortValue: (c) => c.ordinal,
        filterValue: (c) => `${c.name} ${c.comment ?? ""}`,
        cell: (c) => (
          <div className="flex min-w-0 items-center gap-1.5">
            {c.is_primary_key && (
              <Key
                size={11}
                className="shrink-0 text-amber-500"
                aria-label="primary key"
              />
            )}
            <span
              className={cn(
                "truncate font-mono",
                c.is_primary_key
                  ? "font-medium text-foreground"
                  : "text-foreground"
              )}
              title={c.name}
            >
              {c.name}
            </span>
          </div>
        ),
      },
      {
        id: "type",
        header: "type",
        width: 180,
        sortValue: (c) => c.pg_type,
        filterValue: (c) => c.pg_type,
        cell: (c) => (
          <span
            className="truncate font-mono text-muted-foreground"
            title={c.pg_type}
          >
            {c.pg_type}
          </span>
        ),
      },
      {
        id: "nullable",
        header: "nullable",
        width: 100,
        sortValue: (c) => (c.nullable ? 1 : 0),
        cell: (c) =>
          c.nullable ? (
            <span className="text-muted-foreground">yes</span>
          ) : (
            <span className="text-foreground">no</span>
          ),
      },
      {
        id: "default",
        header: "default",
        width: 260,
        filterValue: (c) => c.default_expr ?? undefined,
        cell: (c) =>
          c.default_expr ? (
            <span
              className="truncate font-mono text-muted-foreground"
              title={c.default_expr}
            >
              {c.default_expr}
            </span>
          ) : (
            <span className="text-muted-foreground/40">—</span>
          ),
      },
      {
        id: "comment",
        header: "comment",
        width: 320,
        filterValue: (c) => c.comment ?? undefined,
        cell: (c) =>
          c.comment ? (
            <span className="truncate text-muted-foreground" title={c.comment}>
              {c.comment}
            </span>
          ) : (
            <span className="text-muted-foreground/40">—</span>
          ),
      },
    ],
    []
  );

  return (
    <section className="overflow-hidden rounded-lg border bg-card">
      <header className="flex items-center gap-2 border-b px-4 py-2">
        <h2 className="text-sm font-medium">Columns</h2>
        <span className="text-xs text-muted-foreground">
          {columns.length} columns
          {pkCount > 0 && (
            <>
              {", "}primary key on {pkCount}
            </>
          )}
        </span>
      </header>
      <DataTable
        columns={dataColumns}
        rows={columns}
        rowKey={(c) => c.name}
        defaultSort={{ id: "name", direction: "asc" }}
      />
    </section>
  );
}

function ForeignKeysOutPanel({ rows }: { rows: OutgoingForeignKey[] }) {
  return (
    <section className="overflow-hidden rounded-lg border bg-card">
      <header className="flex items-center gap-2 border-b px-4 py-2">
        <h2 className="text-sm font-medium">Foreign keys</h2>
        <span className="text-xs text-muted-foreground">
          {rows.length} outgoing reference{rows.length === 1 ? "" : "s"}
        </span>
      </header>
      <ul className="divide-y">
        {rows.map((fk) => (
          <li
            key={fk.name}
            className="flex flex-wrap items-center gap-x-2 gap-y-1 px-4 py-2 text-xs"
          >
            <span className="font-mono text-foreground">
              ({fk.columns.join(", ")})
            </span>
            <ChevronRight
              size={12}
              className="text-muted-foreground/60"
              aria-hidden
            />
            <Link
              to={`/schema/tables/${encodeURIComponent(fk.target_table)}`}
              className="font-mono text-foreground underline-offset-2 hover:underline"
            >
              {fk.target_table}
            </Link>
            <span className="font-mono text-muted-foreground">
              ({fk.target_columns.join(", ")})
            </span>
            <span className="ml-auto font-mono text-[10px] text-muted-foreground/70">
              {fk.name}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

function ForeignKeysInPanel({ rows }: { rows: IncomingForeignKey[] }) {
  return (
    <section className="overflow-hidden rounded-lg border bg-card">
      <header className="flex items-center gap-2 border-b px-4 py-2">
        <h2 className="text-sm font-medium">Referenced by</h2>
        <span className="text-xs text-muted-foreground">
          {rows.length} incoming reference{rows.length === 1 ? "" : "s"}
        </span>
      </header>
      <ul className="divide-y">
        {rows.map((fk) => (
          <li
            key={`${fk.source_table}.${fk.name}`}
            className="flex flex-wrap items-center gap-x-2 gap-y-1 px-4 py-2 text-xs"
          >
            <Link
              to={`/schema/tables/${encodeURIComponent(fk.source_table)}`}
              className="font-mono text-foreground underline-offset-2 hover:underline"
            >
              {fk.source_table}
            </Link>
            <span className="font-mono text-muted-foreground">
              ({fk.source_columns.join(", ")})
            </span>
            <ChevronRight
              size={12}
              className="text-muted-foreground/60"
              aria-hidden
            />
            <span className="font-mono text-foreground">
              ({fk.target_columns.join(", ")})
            </span>
            <span className="ml-auto font-mono text-[10px] text-muted-foreground/70">
              {fk.name}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

function TableKpis({ table }: { table: TableStats }) {
  const totalTup = table.n_live_tup + table.n_dead_tup;
  const deadPct = totalTup > 0 ? (table.n_dead_tup / totalTup) * 100 : 0;
  const lastVacuum = table.last_autovacuum ?? table.last_vacuum;

  return (
    <section className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-5">
      <KpiTile
        label="total size"
        value={formatBytes(table.total_bytes)}
        hint={`heap + indexes + toast`}
      />
      <KpiTile label="indexes size" value={formatBytes(table.index_bytes)} />
      <KpiTile
        label="live rows (est)"
        value={table.n_live_tup.toLocaleString()}
        hint={`${table.rows_estimate.toLocaleString()} planner estimate`}
      />
      <KpiTile
        label="dead rows"
        value={`${deadPct.toFixed(1)}%`}
        hint={`${table.n_dead_tup.toLocaleString()} tuples`}
        tone={deadPct >= 50 ? "error" : deadPct >= 20 ? "warn" : "default"}
      />
      <KpiTile
        label="last vacuum"
        value={formatRelative(lastVacuum)}
        hint={
          table.last_analyze
            ? `analyze ${formatRelative(table.last_analyze)}`
            : "analyze: never"
        }
      />
    </section>
  );
}

function IndexesPanel({
  loading,
  error,
  rows,
}: {
  loading: boolean;
  error: Error | null;
  rows: IndexStats[];
}) {
  const columns = useMemo<DataTableColumn<IndexStats>[]>(
    () => [
      {
        id: "index",
        header: "index",
        width: 360,
        sortValue: (i) => i.index,
        filterValue: (i) => i.index,
        cell: (i) => (
          <span className="truncate font-mono text-foreground" title={i.index}>
            {i.index}
          </span>
        ),
      },
      {
        id: "scans",
        header: "scans",
        width: 120,
        align: "right",
        sortValue: (i) => i.idx_scan,
        cell: (i) => (
          <span
            className={cn(
              "tabular-nums",
              i.idx_scan === 0 ? "text-amber-500" : "text-muted-foreground"
            )}
            title={i.idx_scan === 0 ? "Never used since last reset" : undefined}
          >
            {i.idx_scan.toLocaleString()}
          </span>
        ),
      },
      {
        id: "size",
        header: "size",
        width: 120,
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
    <section className="overflow-hidden rounded-lg border bg-card">
      <header className="flex items-center gap-2 border-b px-4 py-2">
        <h2 className="text-sm font-medium">Indexes</h2>
        <span className="text-xs text-muted-foreground">
          {rows.length} on this table
        </span>
      </header>
      {error ? (
        <ErrorBanner
          title="Couldn't read pg_stat_user_indexes"
          body={error.message}
        />
      ) : loading ? (
        <div className="px-4 py-4 text-sm text-muted-foreground">
          Loading indexes…
        </div>
      ) : rows.length === 0 ? (
        <div className="px-4 py-4 text-sm text-muted-foreground">
          No indexes on this table.
        </div>
      ) : (
        <DataTable
          columns={columns}
          rows={rows}
          rowKey={(i) => i.index}
          defaultSort={{ id: "size", direction: "desc" }}
        />
      )}
    </section>
  );
}

function NotFound({ name }: { name: string }) {
  return (
    <div className="rounded-lg border bg-card px-4 py-6 text-sm text-muted-foreground">
      No stats found for{" "}
      <code className="font-mono text-foreground">{name}</code>. It may have
      been dropped or it falls outside the top-200 by size.
    </div>
  );
}

function KpiSkeleton() {
  return (
    <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-5">
      {Array.from({ length: 5 }).map((_, i) => (
        <div key={i} className="h-24 animate-pulse rounded-lg border bg-card" />
      ))}
    </div>
  );
}

/** Postgres identifier needs quoting if it contains uppercase, spaces, or non-ascii. */
function needsQuoting(ident: string): boolean {
  return !/^[a-z_][a-z0-9_]*$/.test(ident);
}
