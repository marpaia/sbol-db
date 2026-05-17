/**
 * System & application observability page.
 *
 * Renders a snapshot of process health (ready, version, uptime), pool
 * capacity, queue state, a rolling 10-minute traffic chart (RPS + p95
 * latency), and recent job history. Backed by `/lab/api/observability/*`
 * and polled every 5 s (summary) / 30 s (jobs).
 */

import { useMemo, useState } from "react";
import {
  Activity,
  AlertTriangle,
  CircleCheck,
  CircleX,
  Loader2,
} from "lucide-react";

import { DataTable, type DataTableColumn } from "@/components/lab/DataTable";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { KpiTile } from "@/components/observability/KpiTile";
import { Sparkline } from "@/components/observability/Sparkline";
import {
  useObservabilitySummary,
  useRecentJobs,
} from "@/hooks/useObservability";
import type {
  BucketSnapshot,
  JobStatus,
  ObservabilitySummary,
  RecentJob,
} from "@/lib/api";
import {
  cn,
  describeError,
  formatMs,
  formatRelative,
  formatUptime,
} from "@/lib/utils";

export default function ObservabilityRoute() {
  const { data: summary, isLoading, error } = useObservabilitySummary();

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-8 px-8 py-10">
        <header>
          <h1 className="text-2xl font-semibold tracking-tight">
            Observability
          </h1>
          <p className="mt-2 text-sm text-muted-foreground">
            Live process health, request traffic, connection pools, and the job
            runner. Sampled every 5 seconds.
          </p>
        </header>

        {error ? (
          <ErrorBanner
            title="Couldn't load summary"
            body={describeError(error)}
          />
        ) : !summary ? (
          isLoading ? (
            <SummarySkeleton />
          ) : null
        ) : (
          <>
            <HealthStrip summary={summary} />
            <Kpis summary={summary} />
            <Charts
              buckets={summary.rolling.buckets}
              bucketSecs={summary.rolling.bucket_secs}
            />
            <Pools summary={summary} />
            <JobsSection summary={summary} />
          </>
        )}
      </div>
    </div>
  );
}

function HealthStrip({ summary }: { summary: ObservabilitySummary }) {
  const { ready, version, uptime_secs, snapshot_at } = summary.health;
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2 rounded-lg border bg-card px-4 py-2.5 text-xs">
      <span className="inline-flex items-center gap-1.5">
        {ready ? (
          <CircleCheck size={12} className="text-emerald-500" />
        ) : (
          <CircleX size={12} className="text-destructive" />
        )}
        <span className={ready ? "text-foreground" : "text-destructive"}>
          {ready ? "ready" : "not ready"}
        </span>
      </span>
      <Sep />
      <Meta label="version">
        <code className="font-mono">{version}</code>
      </Meta>
      <Sep />
      <Meta label="uptime">{formatUptime(uptime_secs)}</Meta>
      <Sep />
      <Meta label="snapshot">{formatRelative(snapshot_at)}</Meta>
    </div>
  );
}

function Sep() {
  return <span className="text-muted-foreground/30">·</span>;
}

function Meta({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <span className="inline-flex items-center gap-1.5">
      <span className="text-muted-foreground/70">{label}</span>
      <span className="text-foreground">{children}</span>
    </span>
  );
}

function Kpis({ summary }: { summary: ObservabilitySummary }) {
  const { rps, p95, errorRate, dbUtil, dbUtilHint } = useMemo(() => {
    const buckets = summary.rolling.buckets;
    const recent = buckets.slice(-Math.max(1, Math.min(30, buckets.length)));
    const recentSecs = Math.max(1, recent.length * summary.rolling.bucket_secs);
    const totalCount = recent.reduce((acc, b) => acc + b.count, 0);
    const totalErrors = recent.reduce((acc, b) => acc + b.error_count, 0);
    const rps = totalCount / recentSecs;
    const lastWithSamples = [...recent].reverse().find((b) => b.count > 0);
    const p95 = lastWithSamples?.p95_ms ?? 0;
    const errorRate = totalCount > 0 ? (totalErrors / totalCount) * 100 : 0;

    const { in_use, size } = summary.pool.api;
    const dbUtil = size > 0 ? (in_use / size) * 100 : 0;
    const dbUtilHint = `${in_use} / ${size} connections in use`;
    return { rps, p95, errorRate, dbUtil, dbUtilHint };
  }, [summary]);

  const queuedJobs = summary.jobs.queue_depth
    .filter((r) => r.status === "queued")
    .reduce((acc, r) => acc + r.count, 0);

  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5">
      <KpiTile
        label="req/sec"
        value={rps < 10 ? rps.toFixed(1) : Math.round(rps).toString()}
        hint={`${summary.rolling.bucket_secs * Math.min(30, summary.rolling.buckets.length)}s window`}
      />
      <KpiTile
        label="p95 latency"
        value={formatMs(p95)}
        hint="last active bucket"
      />
      <KpiTile
        label="db pool"
        value={`${Math.round(dbUtil)}%`}
        hint={dbUtilHint}
        tone={dbUtil >= 80 ? "warn" : "default"}
      />
      <KpiTile
        label="errors (5xx)"
        value={`${errorRate.toFixed(1)}%`}
        hint={`${summary.jobs.failures_24h} job fails / 24h`}
        tone={errorRate > 1 ? "error" : "default"}
      />
      <KpiTile
        label="queued jobs"
        value={queuedJobs}
        hint={oldestQueuedHint(summary)}
        tone={queuedJobs > 50 ? "warn" : "default"}
      />
    </div>
  );
}

function oldestQueuedHint(summary: ObservabilitySummary): string {
  const oldest = summary.jobs.oldest_age[0];
  if (!oldest) return "no waiters";
  return `oldest ${formatMs(oldest.age_secs * 1000)} in ${oldest.queue}`;
}

function Charts({
  buckets,
  bucketSecs,
}: {
  buckets: BucketSnapshot[];
  bucketSecs: number;
}) {
  const counts = buckets.map((b) => b.count);
  const p95s = buckets.map((b) => b.p95_ms);
  const p50s = buckets.map((b) => b.p50_ms);
  const errors = buckets.map((b) => b.error_count);
  const totalWindow = bucketSecs * (buckets.length || 1);
  return (
    <section className="grid gap-4 md:grid-cols-2">
      <ChartPanel
        title="Requests"
        subtitle={`per ${bucketSecs}s bucket · last ${Math.round(totalWindow / 60)}m`}
      >
        <Sparkline
          points={counts}
          height={64}
          color="hsl(var(--primary))"
          ariaLabel="Request rate"
        />
      </ChartPanel>
      <ChartPanel title="Latency" subtitle="p50 (line) · p95 (area)">
        <div className="relative">
          <div className="absolute inset-0 text-muted-foreground/60">
            <Sparkline points={p95s} height={64} ariaLabel="p95 latency" />
          </div>
          <div className="relative text-foreground">
            <Sparkline points={p50s} height={64} ariaLabel="p50 latency" />
          </div>
        </div>
      </ChartPanel>
      {errors.some((v) => v > 0) && (
        <ChartPanel title="5xx errors" subtitle="per bucket">
          <Sparkline
            points={errors}
            height={48}
            color="hsl(var(--destructive))"
            ariaLabel="Error count"
          />
        </ChartPanel>
      )}
    </section>
  );
}

function ChartPanel({
  title,
  subtitle,
  children,
}: {
  title: string;
  subtitle: string;
  children: React.ReactNode;
}) {
  return (
    <section className="rounded-lg border bg-card">
      <header className="flex items-center gap-2 border-b px-4 py-2">
        <h3 className="text-sm font-medium">{title}</h3>
        <span className="text-xs text-muted-foreground">{subtitle}</span>
      </header>
      <div className="px-4 py-3">{children}</div>
    </section>
  );
}

function Pools({ summary }: { summary: ObservabilitySummary }) {
  return (
    <section className="grid gap-3 sm:grid-cols-2">
      <PoolCard label="API pool" stat={summary.pool.api} />
      {summary.pool.worker && (
        <PoolCard label="Worker pool" stat={summary.pool.worker} />
      )}
    </section>
  );
}

function PoolCard({
  label,
  stat,
}: {
  label: string;
  stat: { size: number; idle: number; in_use: number };
}) {
  const utilization = stat.size > 0 ? (stat.in_use / stat.size) * 100 : 0;
  return (
    <div className="rounded-lg border bg-card p-4">
      <div className="flex items-center gap-2 text-xs">
        <span className="text-muted-foreground">{label}</span>
        <span className="ml-auto tabular-nums text-foreground">
          {stat.in_use} / {stat.size}
        </span>
      </div>
      <div className="mt-2 h-2 overflow-hidden rounded-full bg-muted">
        <div
          className={cn(
            "h-full transition-all",
            utilization >= 80
              ? "bg-amber-500"
              : utilization >= 95
                ? "bg-destructive"
                : "bg-primary"
          )}
          style={{ width: `${Math.min(100, Math.max(2, utilization))}%` }}
        />
      </div>
      <div className="mt-1.5 text-[11px] tabular-nums text-muted-foreground">
        {stat.idle} idle · {Math.round(utilization)}% in use
      </div>
    </div>
  );
}

function JobsSection({ summary }: { summary: ObservabilitySummary }) {
  const [queueFilter, setQueueFilter] = useState("");
  const [statusFilter, setStatusFilter] = useState<JobStatus | "">("");

  const queues = useMemo(() => {
    const set = new Set<string>();
    for (const r of summary.jobs.queue_depth) set.add(r.queue);
    for (const r of summary.jobs.oldest_age) set.add(r.queue);
    return [...set].sort();
  }, [summary]);

  const { data: jobs, isLoading } = useRecentJobs({
    limit: 50,
    queue: queueFilter || undefined,
    status: statusFilter || undefined,
  });

  const columns = useMemo<DataTableColumn<RecentJob>[]>(
    () => [
      {
        id: "kind",
        header: "kind",
        width: 200,
        sortValue: (j) => j.kind,
        filterValue: (j) => `${j.kind} ${j.id}`,
        cell: (j) => (
          <div className="min-w-0">
            <div className="truncate text-foreground">{j.kind}</div>
            <div className="truncate font-mono text-[10px] text-muted-foreground/70">
              {j.id}
            </div>
          </div>
        ),
      },
      {
        id: "queue",
        header: "queue",
        width: 120,
        sortValue: (j) => j.queue,
        filterValue: (j) => j.queue,
        cell: (j) => (
          <span className="truncate font-mono text-[11px] text-muted-foreground">
            {j.queue}
          </span>
        ),
      },
      {
        id: "status",
        header: "status",
        width: 110,
        sortValue: (j) => j.status,
        filterValue: (j) => j.status,
        cell: (j) => <StatusPill status={j.status} />,
      },
      {
        id: "attempts",
        header: "attempts",
        width: 90,
        align: "right",
        sortValue: (j) => j.attempts,
        cell: (j) => (
          <span className="tabular-nums text-muted-foreground">
            {j.attempts}
            <span className="text-muted-foreground/40">/{j.max_attempts}</span>
          </span>
        ),
      },
      {
        id: "duration",
        header: "duration",
        width: 110,
        align: "right",
        sortValue: (j) => jobDurationMs(j) ?? -1,
        cell: (j) => {
          const ms = jobDurationMs(j);
          return (
            <span className="tabular-nums text-muted-foreground">
              {ms !== null ? formatMs(ms) : "—"}
            </span>
          );
        },
      },
      {
        id: "started",
        header: "started",
        width: 110,
        sortValue: (j) => new Date(j.started_at ?? j.created_at).getTime() || 0,
        cell: (j) => (
          <span className="text-muted-foreground">
            {formatRelative(j.started_at ?? j.created_at)}
          </span>
        ),
      },
      {
        id: "error",
        header: "error",
        width: 280,
        filterValue: (j) => j.error ?? undefined,
        cell: (j) =>
          j.error ? (
            <span className="truncate text-destructive/80" title={j.error}>
              {j.error}
            </span>
          ) : (
            <span className="text-muted-foreground/40">—</span>
          ),
      },
    ],
    []
  );

  return (
    <section className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <h2 className="text-sm font-medium">Recent jobs</h2>
        <span className="text-xs text-muted-foreground">last 50</span>
        <div className="ml-auto flex items-center gap-2">
          <FilterSelect
            value={queueFilter}
            onChange={setQueueFilter}
            options={[
              { value: "", label: "all queues" },
              ...queues.map((q) => ({ value: q, label: q })),
            ]}
          />
          <FilterSelect
            value={statusFilter}
            onChange={(v) => setStatusFilter(v as JobStatus | "")}
            options={[
              { value: "", label: "all status" },
              { value: "queued", label: "queued" },
              { value: "running", label: "running" },
              { value: "succeeded", label: "succeeded" },
              { value: "failed", label: "failed" },
              { value: "cancelled", label: "cancelled" },
              { value: "dead", label: "dead" },
            ]}
          />
        </div>
      </div>

      <div className="overflow-hidden rounded-lg border bg-card">
        {isLoading && !jobs ? (
          <JobsSkeleton />
        ) : !jobs ? null : (
          <DataTable
            columns={columns}
            rows={jobs}
            rowKey={(j) => j.id}
            filterable
            defaultSort={{ id: "started", direction: "desc" }}
            emptyMessage="No jobs match the current filters."
          />
        )}
      </div>

      <QueueHealth summary={summary} />
    </section>
  );
}

function FilterSelect({
  value,
  onChange,
  options,
}: {
  value: string;
  onChange: (v: string) => void;
  options: { value: string; label: string }[];
}) {
  return (
    <select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="rounded-md border bg-background px-2 py-1 text-xs text-foreground outline-none focus:ring-1 focus:ring-ring"
    >
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

function jobDurationMs(job: RecentJob): number | null {
  if (job.started_at && job.finished_at) {
    return (
      new Date(job.finished_at).getTime() - new Date(job.started_at).getTime()
    );
  }
  if (job.started_at) {
    return Date.now() - new Date(job.started_at).getTime();
  }
  return null;
}

function StatusPill({ status }: { status: JobStatus }) {
  const tone =
    status === "succeeded"
      ? "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400"
      : status === "running"
        ? "bg-primary/10 text-primary"
        : status === "queued"
          ? "bg-muted text-muted-foreground"
          : status === "cancelled"
            ? "bg-muted text-muted-foreground"
            : "bg-destructive/10 text-destructive";
  return (
    <span
      className={cn(
        "inline-flex items-center rounded-sm px-1.5 py-0.5 text-[10px] uppercase tracking-wide",
        tone
      )}
    >
      {status}
    </span>
  );
}

function QueueHealth({ summary }: { summary: ObservabilitySummary }) {
  if (
    summary.jobs.queue_depth.length === 0 &&
    summary.jobs.oldest_age.length === 0
  ) {
    return null;
  }
  return (
    <section className="rounded-lg border bg-card">
      <header className="flex items-center gap-2 border-b px-4 py-2">
        <h3 className="text-sm font-medium">Queue health</h3>
        <span className="text-xs text-muted-foreground">
          live snapshot · {summary.jobs.failures_24h} failure
          {summary.jobs.failures_24h === 1 ? "" : "s"} in last 24h
        </span>
      </header>
      <div className="grid gap-x-4 gap-y-1 px-4 py-3 text-xs sm:grid-cols-2">
        {summary.jobs.queue_depth.length === 0 ? (
          <div className="text-muted-foreground">all queues empty</div>
        ) : (
          summary.jobs.queue_depth.map((r) => (
            <div
              key={`${r.queue}-${r.status}`}
              className="flex items-center gap-2"
            >
              <span className="font-mono text-muted-foreground">{r.queue}</span>
              <span className="text-muted-foreground/60">·</span>
              <span className="text-foreground">{r.status}</span>
              <span className="ml-auto tabular-nums text-foreground">
                {r.count.toLocaleString()}
              </span>
            </div>
          ))
        )}
      </div>
    </section>
  );
}

function SummarySkeleton() {
  return (
    <div className="space-y-4">
      <div className="h-10 animate-pulse rounded-lg border bg-card" />
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-5">
        {Array.from({ length: 5 }).map((_, i) => (
          <div
            key={i}
            className="h-24 animate-pulse rounded-lg border bg-card"
          />
        ))}
      </div>
      <div className="flex items-center justify-center gap-2 py-6 text-sm text-muted-foreground">
        <Loader2 size={14} className="animate-spin" />
        <span>Loading metrics…</span>
        <Activity size={14} className="text-muted-foreground/40" />
        <AlertTriangle size={14} className="text-muted-foreground/40" />
      </div>
    </div>
  );
}

function JobsSkeleton() {
  return (
    <div className="divide-y">
      {Array.from({ length: 4 }).map((_, i) => (
        <div key={i} className="flex items-center gap-3 px-4 py-3">
          <div className="h-3 w-32 animate-pulse rounded bg-muted" />
          <div className="h-3 w-16 animate-pulse rounded bg-muted" />
          <div className="h-3 flex-1 animate-pulse rounded bg-muted" />
        </div>
      ))}
    </div>
  );
}
