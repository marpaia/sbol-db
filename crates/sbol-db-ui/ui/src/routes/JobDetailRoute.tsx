/**
 * Per-job drill-down. Reached by clicking a row in the Recent jobs
 * table on `/observability` at `/observability/jobs/:id`.
 *
 * Shows job-level metadata (kind, queue, status, attempts), timing,
 * the inbound payload, and the result or error. Polls every second
 * while the job is still pending so the page reflects worker progress
 * without manual refresh.
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronLeft, Octagon } from "lucide-react";
import { Link, useParams } from "react-router-dom";

import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { JobStatusBadge } from "@/components/observability/JobStatusBadge";
import { KpiTile } from "@/components/observability/KpiTile";
import {
  useCancelJob,
  useJob,
  useJobAttempts,
  useJobLogs,
} from "@/hooks/useObservability";
import type { JobAttempt, JobLogRecord, RecentJob } from "@/lib/api";
import { describeError, formatMs } from "@/lib/utils";

export default function JobDetailRoute() {
  const params = useParams<{ id: string }>();
  const id = params.id ?? "";
  const { data: job, isLoading, error } = useJob(id);
  const attemptsQuery = useJobAttempts(id, job?.status);
  const logsQuery = useJobLogs(id, job?.status);
  const cancel = useCancelJob();
  const now = useLiveNow(job?.status);

  const cancelMessage = useMemo(() => {
    if (cancel.isError) return describeError(cancel.error);
    if (cancel.data && cancel.data.cancelled === false) {
      return "Job was already in a terminal state; nothing to cancel.";
    }
    return null;
  }, [cancel.data, cancel.error, cancel.isError]);

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-6 px-8 py-10">
        <Link
          to="/observability/jobs"
          className="inline-flex items-center gap-1 text-xs text-muted-foreground transition-colors hover:text-foreground"
        >
          <ChevronLeft size={12} />
          Jobs
        </Link>

        {error ? (
          <ErrorBanner title="Couldn't load job" body={describeError(error)} />
        ) : isLoading && !job ? (
          <Skeleton />
        ) : !job ? (
          <NotFound id={id} />
        ) : (
          <>
            <Header
              job={job}
              onCancel={() => cancel.mutate(job.id)}
              canCancel={
                (job.status === "queued" || job.status === "running") &&
                !cancel.isPending
              }
              cancelling={cancel.isPending}
              cancelMessage={cancelMessage}
            />
            <JobKpis job={job} now={now} />
            <TimingPanel job={job} now={now} />
            <PayloadPanel payload={job.payload} />
            {job.status === "succeeded" && job.result !== null && (
              <ResultPanel result={job.result} />
            )}
            {job.error && <ErrorPanel error={job.error} />}
            <LogsPanel
              loading={logsQuery.isLoading && !logsQuery.data}
              error={logsQuery.error}
              logs={logsQuery.data ?? null}
            />
            <AttemptsPanel
              loading={attemptsQuery.isLoading && !attemptsQuery.data}
              error={attemptsQuery.error}
              attempts={attemptsQuery.data ?? null}
              now={now}
            />
          </>
        )}
      </div>
    </div>
  );
}

function AttemptsPanel({
  loading,
  error,
  attempts,
  now,
}: {
  loading: boolean;
  error: unknown;
  attempts: JobAttempt[] | null;
  now: number;
}) {
  if (error) {
    return (
      <ErrorBanner title="Couldn't load attempts" body={describeError(error)} />
    );
  }
  return (
    <section className="overflow-hidden rounded-lg border bg-card">
      <header className="flex items-center gap-2 border-b px-4 py-2">
        <h2 className="text-sm font-medium">Attempts</h2>
        {attempts && (
          <span className="text-xs text-muted-foreground">
            {attempts.length} {attempts.length === 1 ? "attempt" : "attempts"}{" "}
            recorded
          </span>
        )}
      </header>
      {loading ? (
        <div className="px-4 py-4 text-sm text-muted-foreground">
          Loading attempts…
        </div>
      ) : !attempts || attempts.length === 0 ? (
        <div className="px-4 py-4 text-sm text-muted-foreground">
          No attempts recorded yet. Attempts are written when a worker dequeues
          the job.
        </div>
      ) : (
        <ul className="divide-y">
          {attempts.map((a) => (
            <AttemptRow key={a.id} attempt={a} now={now} />
          ))}
        </ul>
      )}
    </section>
  );
}

function LogsPanel({
  loading,
  error,
  logs,
}: {
  loading: boolean;
  error: unknown;
  logs: JobLogRecord[] | null;
}) {
  const bottomRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ block: "end" });
  }, [logs?.length]);

  if (error) {
    return (
      <ErrorBanner title="Couldn't load job logs" body={describeError(error)} />
    );
  }
  return (
    <section className="overflow-hidden rounded-lg border bg-card">
      <header className="flex items-center gap-2 border-b px-4 py-2">
        <h2 className="text-sm font-medium">Logs</h2>
        {logs && (
          <span className="text-xs text-muted-foreground">
            {logs.length} {logs.length === 1 ? "event" : "events"}
          </span>
        )}
      </header>
      <div className="max-h-80 overflow-auto bg-zinc-950 px-4 py-3 font-mono text-[11px] leading-relaxed text-zinc-100">
        {loading ? (
          <div className="text-zinc-400">Waiting for log events…</div>
        ) : !logs || logs.length === 0 ? (
          <div className="text-zinc-400">
            No job logs recorded yet. New worker events will appear here.
          </div>
        ) : (
          <div className="space-y-1">
            {logs.map((log) => (
              <LogLine key={log.id} log={log} />
            ))}
            <div ref={bottomRef} />
          </div>
        )}
      </div>
    </section>
  );
}

function LogLine({ log }: { log: JobLogRecord }) {
  const fields = formatLogFields(log.fields);
  return (
    <div className="grid gap-2 sm:grid-cols-[170px_52px_1fr]">
      <span className="text-zinc-500">{formatLogTime(log.created_at)}</span>
      <span className={logLevelClass(log.level)}>{log.level}</span>
      <span className="min-w-0 break-words">
        {log.attempt_no !== null && (
          <span className="mr-2 text-zinc-500">attempt={log.attempt_no}</span>
        )}
        <span>{log.message}</span>
        {fields && <span className="ml-2 text-zinc-400">{fields}</span>}
      </span>
    </div>
  );
}

function AttemptRow({ attempt, now }: { attempt: JobAttempt; now: number }) {
  const duration =
    attempt.finished_at && attempt.started_at
      ? new Date(attempt.finished_at).getTime() -
        new Date(attempt.started_at).getTime()
      : attempt.started_at
        ? now - new Date(attempt.started_at).getTime()
        : null;
  return (
    <li className="space-y-2 px-4 py-3 text-xs">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className="font-mono text-muted-foreground">
          #{attempt.attempt_no}
        </span>
        <JobStatusBadge status={attempt.status} />
        <span className="text-muted-foreground/60">·</span>
        <span className="text-muted-foreground">
          {formatRelativeAt(attempt.started_at, now)}
        </span>
        <span className="text-muted-foreground/60">·</span>
        <span className="tabular-nums text-muted-foreground">
          {duration !== null
            ? attempt.finished_at
              ? formatMs(duration)
              : `${formatMs(duration)} (running)`
            : "—"}
        </span>
        <span
          className="ml-auto truncate font-mono text-[10px] text-muted-foreground/70"
          title={attempt.worker_id}
        >
          {attempt.worker_id}
        </span>
      </div>
      {attempt.error && (
        <pre className="max-h-32 overflow-auto whitespace-pre-wrap rounded-md border border-destructive/20 bg-destructive/5 px-2 py-1.5 font-mono text-[11px] leading-snug text-destructive/90">
          {attempt.error}
        </pre>
      )}
    </li>
  );
}

function Header({
  job,
  onCancel,
  canCancel,
  cancelling,
  cancelMessage,
}: {
  job: RecentJob;
  onCancel: () => void;
  canCancel: boolean;
  cancelling: boolean;
  cancelMessage: string | null;
}) {
  return (
    <div className="space-y-2">
      <header className="flex flex-wrap items-start justify-between gap-4">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <h1 className="truncate text-xl font-semibold tracking-tight">
              {job.kind}
            </h1>
            <JobStatusBadge status={job.status} />
          </div>
          <div className="mt-1 truncate font-mono text-[11px] text-muted-foreground">
            {job.id}
          </div>
        </div>
        {(canCancel || cancelling) && (
          <button
            type="button"
            onClick={onCancel}
            disabled={!canCancel}
            className="inline-flex items-center gap-1.5 rounded-md border border-destructive/40 bg-background px-3 py-1.5 text-sm font-medium text-destructive transition-colors hover:bg-destructive/10 disabled:opacity-50"
          >
            <Octagon size={12} />
            {cancelling ? "Cancelling…" : "Cancel"}
          </button>
        )}
      </header>
      {cancelMessage && (
        <div className="rounded-md border border-amber-500/30 bg-amber-500/5 px-3 py-2 text-xs text-amber-700 dark:text-amber-400">
          {cancelMessage}
        </div>
      )}
    </div>
  );
}

function JobKpis({ job, now }: { job: RecentJob; now: number }) {
  const duration = jobDurationMs(job, now);
  const durationValue =
    duration === null
      ? "—"
      : job.finished_at
        ? formatMs(duration)
        : `${formatMs(duration)} (running)`;
  return (
    <section className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
      <KpiTile label="status" value={job.status} />
      <KpiTile
        label="attempts"
        value={`${job.attempts} / ${job.max_attempts}`}
        tone={job.attempts >= job.max_attempts ? "warn" : "default"}
      />
      <KpiTile label="queue" value={job.queue} />
      <KpiTile label="duration" value={durationValue} />
    </section>
  );
}

function TimingPanel({ job, now }: { job: RecentJob; now: number }) {
  const rows: { label: string; value: string }[] = [
    { label: "created", value: formatTime(job.created_at, now) },
    { label: "started", value: formatTime(job.started_at, now) },
    { label: "finished", value: formatTime(job.finished_at, now) },
    { label: "available at", value: formatTime(job.available_at, now) },
    { label: "lease expires", value: formatTime(job.lease_expires_at, now) },
    { label: "leased by", value: job.leased_by ?? "—" },
    { label: "priority", value: String(job.priority) },
    { label: "correlation id", value: job.correlation_id ?? "—" },
    { label: "parent job", value: job.parent_job_id ?? "—" },
    { label: "idempotency key", value: job.idempotency_key ?? "—" },
  ];
  return (
    <section className="overflow-hidden rounded-lg border bg-card">
      <header className="flex items-center gap-2 border-b px-4 py-2">
        <h2 className="text-sm font-medium">Timing & identity</h2>
      </header>
      <dl className="grid gap-x-6 gap-y-1 px-4 py-3 text-xs sm:grid-cols-2">
        {rows.map((r) => (
          <div key={r.label} className="flex items-baseline gap-2">
            <dt className="w-32 shrink-0 text-muted-foreground">{r.label}</dt>
            <dd className="truncate font-mono text-foreground" title={r.value}>
              {r.value}
            </dd>
          </div>
        ))}
      </dl>
    </section>
  );
}

function PayloadPanel({ payload }: { payload: unknown }) {
  return (
    <section className="overflow-hidden rounded-lg border bg-card">
      <header className="flex items-center gap-2 border-b px-4 py-2">
        <h2 className="text-sm font-medium">Payload</h2>
      </header>
      <pre className="max-h-96 overflow-auto px-4 py-3 font-mono text-[11px] leading-relaxed text-foreground">
        {formatJson(payload)}
      </pre>
    </section>
  );
}

function ResultPanel({ result }: { result: unknown }) {
  return (
    <section className="overflow-hidden rounded-lg border border-emerald-500/30 bg-emerald-500/5">
      <header className="flex items-center gap-2 border-b border-emerald-500/30 px-4 py-2">
        <h2 className="text-sm font-medium text-emerald-700 dark:text-emerald-400">
          Result
        </h2>
      </header>
      <pre className="max-h-96 overflow-auto px-4 py-3 font-mono text-[11px] leading-relaxed text-foreground">
        {formatJson(result)}
      </pre>
    </section>
  );
}

function ErrorPanel({ error }: { error: string }) {
  return (
    <section className="overflow-hidden rounded-lg border border-destructive/40 bg-destructive/5">
      <header className="flex items-center gap-2 border-b border-destructive/30 px-4 py-2">
        <h2 className="text-sm font-medium text-destructive">Error</h2>
      </header>
      <pre className="max-h-96 overflow-auto whitespace-pre-wrap px-4 py-3 font-mono text-[11px] leading-relaxed text-destructive/90">
        {error}
      </pre>
    </section>
  );
}

function Skeleton() {
  return (
    <div className="space-y-4">
      <div className="h-12 animate-pulse rounded-lg bg-card" />
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

function NotFound({ id }: { id: string }) {
  return (
    <div className="rounded-lg border bg-card px-4 py-6 text-sm text-muted-foreground">
      No job found with id{" "}
      <code className="font-mono text-foreground">{id}</code>. It may have been
      pruned, or the id is wrong.
    </div>
  );
}

function jobDurationMs(job: RecentJob, now: number): number | null {
  if (job.started_at && job.finished_at) {
    return (
      new Date(job.finished_at).getTime() - new Date(job.started_at).getTime()
    );
  }
  if (job.started_at) {
    return now - new Date(job.started_at).getTime();
  }
  return null;
}

function formatTime(when: string | null | undefined, now: number): string {
  if (!when) return "—";
  const date = new Date(when);
  if (Number.isNaN(date.getTime())) return "—";
  return `${date
    .toISOString()
    .replace("T", " ")
    .replace(/\.\d+Z$/, "Z")}  ·  ${formatRelativeAt(date, now)}`;
}

function formatJson(value: unknown): string {
  if (value === null || value === undefined) return "—";
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function formatLogTime(when: string): string {
  const date = new Date(when);
  if (Number.isNaN(date.getTime())) return when;
  return date
    .toISOString()
    .replace("T", " ")
    .replace(/\.\d+Z$/, "Z");
}

function formatLogFields(fields: unknown): string {
  if (!fields || typeof fields !== "object") return "";
  const entries = Object.entries(fields as Record<string, unknown>).filter(
    ([, value]) => value !== null && value !== undefined
  );
  if (entries.length === 0) return "";
  return entries
    .map(([key, value]) => `${key}=${formatLogValue(value)}`)
    .join(" ");
}

function formatLogValue(value: unknown): string {
  if (typeof value === "string") {
    return value.includes(" ") ? JSON.stringify(value) : value;
  }
  return JSON.stringify(value);
}

function logLevelClass(level: JobLogRecord["level"]): string {
  switch (level) {
    case "debug":
      return "text-zinc-500";
    case "warn":
      return "text-amber-300";
    case "error":
      return "text-red-300";
    case "info":
      return "text-sky-300";
  }
}

function useLiveNow(status: RecentJob["status"] | undefined): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (status !== "queued" && status !== "running") {
      setNow(Date.now());
      return;
    }
    const interval = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(interval);
  }, [status]);
  return now;
}

function formatRelativeAt(
  when: string | Date | number | null | undefined,
  now: number
): string {
  if (when === null || when === undefined) return "—";
  const then =
    typeof when === "number"
      ? when
      : (typeof when === "string" ? new Date(when) : when).getTime();
  if (Number.isNaN(then)) return "—";
  const seconds = Math.max(0, Math.floor((now - then) / 1000));
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}
