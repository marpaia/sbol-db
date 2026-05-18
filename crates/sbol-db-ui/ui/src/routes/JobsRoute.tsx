/**
 * Background job runner page. Lists the most recent jobs across all
 * queues with status/queue filters; rows link out to the per-job
 * detail page. Queue health (per-queue depth + last-24h failures) is
 * shown below the table as a compact live snapshot. Polled every 30 s
 * via `useRecentJobs`; the summary tile is polled every 5 s via
 * `useObservabilitySummary`.
 */

import { useMemo, useState } from "react";
import { Plus } from "lucide-react";
import { useNavigate } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";

import { DataTable, type DataTableColumn } from "@/components/lab/DataTable";
import { EnqueueJobDialog } from "@/components/lab/EnqueueJobDialog";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { JobStatusBadge } from "@/components/observability/JobStatusBadge";
import {
  useObservabilitySummary,
  useRecentJobs,
} from "@/hooks/useObservability";
import type { JobStatus, ObservabilitySummary, RecentJob } from "@/lib/api";
import { describeError, formatMs, formatRelative } from "@/lib/utils";

export default function JobsRoute() {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { data: summary } = useObservabilitySummary();
  const [queueFilter, setQueueFilter] = useState("");
  const [statusFilter, setStatusFilter] = useState<JobStatus | "">("");
  const [enqueueOpen, setEnqueueOpen] = useState(false);

  const queues = useMemo(() => {
    if (!summary) return [];
    const set = new Set<string>();
    for (const r of summary.jobs.queue_depth) set.add(r.queue);
    for (const r of summary.jobs.oldest_age) set.add(r.queue);
    return [...set].sort();
  }, [summary]);

  const {
    data: jobs,
    isLoading,
    error,
  } = useRecentJobs({
    limit: 100,
    queue: queueFilter || undefined,
    status: statusFilter || undefined,
  });

  const columns = useMemo<DataTableColumn<RecentJob>[]>(
    () => [
      {
        id: "kind",
        header: "kind",
        width: 220,
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
        cell: (j) => <JobStatusBadge status={j.status} />,
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
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-6 px-8 py-10">
        <header className="flex flex-wrap items-start justify-between gap-4">
          <div>
            <h1 className="text-2xl font-semibold tracking-tight">Jobs</h1>
            <p className="mt-2 text-sm text-muted-foreground">
              Background work queue. Click any row to inspect its payload,
              timing, and result; running or queued jobs can be cancelled
              from their detail page.
            </p>
          </div>
          <button
            type="button"
            onClick={() => setEnqueueOpen(true)}
            className="inline-flex items-center gap-1.5 rounded-md border bg-background px-3 py-1.5 text-sm font-medium transition-colors hover:bg-accent"
          >
            <Plus size={14} />
            Enqueue job
          </button>
        </header>

        {error ? (
          <ErrorBanner
            title="Couldn't load jobs"
            body={describeError(error)}
          />
        ) : (
          <>
            <section className="space-y-3">
              <div className="flex flex-wrap items-center gap-2">
                <h2 className="text-sm font-medium">Recent jobs</h2>
                <span className="text-xs text-muted-foreground">last 100</span>
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
                    onRowClick={(j) =>
                      navigate(`/observability/jobs/${j.id}`)
                    }
                  />
                )}
              </div>
            </section>

            {summary && <QueueHealth summary={summary} />}
          </>
        )}
      </div>

      <EnqueueJobDialog
        open={enqueueOpen}
        onOpenChange={setEnqueueOpen}
        onEnqueued={(result) => {
          qc.invalidateQueries({ queryKey: ["lab", "obs", "jobs", "recent"] });
          qc.invalidateQueries({ queryKey: ["lab", "obs", "summary"] });
          navigate(`/observability/jobs/${result.job.id}`);
        }}
      />
    </div>
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

function QueueHealth({ summary }: { summary: ObservabilitySummary }) {
  if (
    summary.jobs.queue_depth.length === 0 &&
    summary.jobs.oldest_age.length === 0
  ) {
    return (
      <section className="rounded-lg border bg-card px-4 py-3 text-xs text-muted-foreground">
        All queues empty.{" "}
        {summary.jobs.failures_24h > 0
          ? `${summary.jobs.failures_24h} failure${summary.jobs.failures_24h === 1 ? "" : "s"} in last 24h.`
          : "No failures in last 24h."}
      </section>
    );
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

function JobsSkeleton() {
  return (
    <div className="divide-y">
      {Array.from({ length: 6 }).map((_, i) => (
        <div key={i} className="flex items-center gap-3 px-4 py-3">
          <div className="h-3 w-32 animate-pulse rounded bg-muted" />
          <div className="h-3 w-16 animate-pulse rounded bg-muted" />
          <div className="h-3 flex-1 animate-pulse rounded bg-muted" />
        </div>
      ))}
    </div>
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
