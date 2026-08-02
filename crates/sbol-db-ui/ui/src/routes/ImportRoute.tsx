import { useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  Check,
  DownloadCloud,
  ExternalLink,
  Globe2,
  Loader2,
  TriangleAlert,
} from "lucide-react";
import { useNavigate } from "react-router-dom";

import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { Button } from "@/components/ui/button";
import {
  enqueueRemoteImport,
  importFormatLabel,
  type EnqueueJobResult,
  type ImportDocumentFormat,
  type ImportRemoteDocumentPayload,
} from "@/lib/api";
import { adminPath } from "@/lib/routes";
import { describeError } from "@/lib/utils";

type ImportTarget = {
  id: string;
  title: string;
  description: string;
  url: string;
  format: ImportDocumentFormat;
  namespace?: string;
};

type ImportState =
  | { kind: "queued"; jobId: string; deduplicated: boolean }
  | { kind: "failed"; message: string };

export default function ImportRoute() {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [states, setStates] = useState<Record<string, ImportState>>({});
  const [submitting, setSubmitting] = useState(false);
  const [pageError, setPageError] = useState<string | null>(null);
  const [lastJob, setLastJob] = useState<EnqueueJobResult | null>(null);
  const [urlList, setUrlList] = useState("");
  const [urlFormat, setUrlFormat] = useState<ImportDocumentFormat>("rdfxml");
  const [urlNamespace, setUrlNamespace] = useState("");

  const targets = useMemo(
    () => parseUrlTargets(urlList, urlFormat, urlNamespace.trim() || undefined),
    [urlList, urlFormat, urlNamespace]
  );
  const queuedCount = Object.values(states).filter(
    (state) => state.kind === "queued"
  ).length;
  const failedCount = Object.values(states).filter(
    (state) => state.kind === "failed"
  ).length;

  const enqueueTargets = async () => {
    if (targets.length === 0) return;
    setSubmitting(true);
    setPageError(null);
    let newest: EnqueueJobResult | null = null;

    for (const target of targets) {
      try {
        const result = await enqueueRemoteImport(toRemotePayload(target), {
          max_attempts: 3,
          idempotency_key: `document-import:${target.format}:${target.url}`,
        });
        newest = result;
        setStates((prev) => ({
          ...prev,
          [target.id]: {
            kind: "queued",
            jobId: result.job.id,
            deduplicated: result.deduplicated,
          },
        }));
      } catch (err) {
        const message = describeError(err);
        setStates((prev) => ({
          ...prev,
          [target.id]: { kind: "failed", message },
        }));
        setPageError(message);
      }
    }

    if (newest) setLastJob(newest);
    qc.invalidateQueries({ queryKey: ["lab", "obs", "jobs", "recent"] });
    qc.invalidateQueries({ queryKey: ["lab", "obs", "summary"] });
    setSubmitting(false);
  };

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-6 px-8 py-10">
        <header className="flex flex-wrap items-start justify-between gap-4">
          <div>
            <h1 className="text-2xl font-semibold tracking-tight">Import</h1>
            <p className="mt-2 max-w-2xl text-sm text-muted-foreground">
              Queue SBOL documents from public HTTPS URLs.
            </p>
          </div>
          {lastJob && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() =>
                navigate(adminPath(`/observability/jobs/${lastJob.job.id}`))
              }
            >
              <ExternalLink size={14} />
              Latest job
            </Button>
          )}
        </header>

        {pageError && (
          <ErrorBanner title="Import enqueue failed" body={pageError} />
        )}

        <section className="grid gap-3 sm:grid-cols-3">
          <MetricCard label="Documents" value={targets.length} />
          <MetricCard label="Queued jobs" value={queuedCount} />
          <MetricCard label="Failed enqueues" value={failedCount} />
        </section>

        <UrlListPanel
          value={urlList}
          onChange={setUrlList}
          format={urlFormat}
          onFormat={setUrlFormat}
          namespace={urlNamespace}
          onNamespace={setUrlNamespace}
          targets={targets}
          states={states}
          submitting={submitting}
          onEnqueue={enqueueTargets}
          onOpenJob={(jobId) =>
            navigate(adminPath(`/observability/jobs/${jobId}`))
          }
        />
      </div>
    </div>
  );
}

function UrlListPanel({
  value,
  onChange,
  format,
  onFormat,
  namespace,
  onNamespace,
  targets,
  states,
  submitting,
  onEnqueue,
  onOpenJob,
}: {
  value: string;
  onChange: (value: string) => void;
  format: ImportDocumentFormat;
  onFormat: (value: ImportDocumentFormat) => void;
  namespace: string;
  onNamespace: (value: string) => void;
  targets: ImportTarget[];
  states: Record<string, ImportState>;
  submitting: boolean;
  onEnqueue: () => void;
  onOpenJob: (jobId: string) => void;
}) {
  return (
    <section className="rounded-lg border bg-card">
      <div className="flex flex-wrap items-center gap-3 border-b px-4 py-3">
        <div className="flex items-center gap-2">
          <Globe2 size={16} className="text-muted-foreground" />
          <h2 className="text-sm font-medium">Document URLs</h2>
        </div>
        <span className="ml-auto text-xs tabular-nums text-muted-foreground">
          {targets.length.toLocaleString()} inputs
        </span>
      </div>

      <div className="grid gap-4 p-4 lg:grid-cols-[minmax(0,1fr)_280px]">
        <label className="grid gap-1.5">
          <span className="text-xs font-medium text-muted-foreground">
            URLs
          </span>
          <textarea
            value={value}
            onChange={(event) => onChange(event.target.value)}
            placeholder="https://example.org/designs/example.xml"
            className="min-h-64 resize-y rounded-md border bg-background p-3 font-mono text-xs text-foreground outline-none focus:ring-1 focus:ring-ring"
          />
          <span className="text-xs text-muted-foreground">
            Enter one URL per line. Optionally add a format and display name
            after the URL.
          </span>
        </label>

        <div className="space-y-3">
          <FormatSelect value={format} onChange={onFormat} />
          <TextField
            label="Namespace"
            value={namespace}
            onChange={onNamespace}
            placeholder="https://example.org/imports"
            mono
          />
          <Button
            type="button"
            className="w-full"
            disabled={submitting || targets.length === 0}
            onClick={onEnqueue}
          >
            {submitting ? (
              <Loader2 size={14} className="animate-spin" />
            ) : (
              <DownloadCloud size={14} />
            )}
            Enqueue {targets.length.toLocaleString()}
          </Button>
        </div>
      </div>

      {targets.length > 0 && (
        <div className="divide-y border-t">
          {targets.map((target) => (
            <div
              key={target.id}
              className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-3 px-4 py-3"
            >
              <div className="min-w-0">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="text-sm font-medium">{target.title}</span>
                  <span className="rounded border bg-background px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
                    {target.format}
                  </span>
                </div>
                <div className="mt-1 truncate font-mono text-[11px] text-muted-foreground">
                  {target.url}
                </div>
              </div>
              <StatusPill state={states[target.id]} onOpenJob={onOpenJob} />
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

function StatusPill({
  state,
  onOpenJob,
}: {
  state?: ImportState;
  onOpenJob: (jobId: string) => void;
}) {
  if (!state) {
    return <span className="text-xs text-muted-foreground/50">idle</span>;
  }
  if (state.kind === "failed") {
    return (
      <span
        title={state.message}
        className="inline-flex items-center gap-1 rounded-full border border-destructive/30 bg-destructive/10 px-2 py-1 text-xs text-destructive"
      >
        <TriangleAlert size={12} />
        failed
      </span>
    );
  }
  return (
    <button
      type="button"
      onClick={() => onOpenJob(state.jobId)}
      className="inline-flex items-center gap-1 rounded-full border border-emerald-500/30 bg-emerald-500/10 px-2 py-1 text-xs text-emerald-700 transition-colors hover:bg-emerald-500/20 dark:text-emerald-300"
    >
      <Check size={12} />
      {state.deduplicated ? "deduped" : "queued"}
    </button>
  );
}

function MetricCard({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-lg border bg-card px-4 py-3">
      <div className="text-xs uppercase tracking-wider text-muted-foreground">
        {label}
      </div>
      <div className="mt-1 text-2xl font-semibold tabular-nums">
        {value.toLocaleString()}
      </div>
    </div>
  );
}

function TextField({
  label,
  value,
  onChange,
  placeholder,
  mono,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  mono?: boolean;
}) {
  return (
    <label className="grid gap-1.5">
      <span className="text-xs font-medium text-muted-foreground">{label}</span>
      <input
        value={value}
        onChange={(event) => onChange(event.target.value)}
        placeholder={placeholder}
        className={`rounded-md border bg-background px-3 py-2 text-sm outline-none focus:ring-1 focus:ring-ring ${
          mono ? "font-mono text-xs" : ""
        }`}
      />
    </label>
  );
}

function FormatSelect({
  value,
  onChange,
}: {
  value: ImportDocumentFormat;
  onChange: (value: ImportDocumentFormat) => void;
}) {
  const formats: ImportDocumentFormat[] = [
    "rdfxml",
    "turtle",
    "jsonld",
    "ntriples",
    "genbank",
    "fasta",
  ];
  return (
    <label className="grid gap-1.5">
      <span className="text-xs font-medium text-muted-foreground">Format</span>
      <select
        value={value}
        onChange={(event) =>
          onChange(event.target.value as ImportDocumentFormat)
        }
        className="rounded-md border bg-background px-3 py-2 text-sm outline-none focus:ring-1 focus:ring-ring"
      >
        {formats.map((option) => (
          <option key={option} value={option}>
            {importFormatLabel(option)}
          </option>
        ))}
      </select>
    </label>
  );
}

function parseUrlTargets(
  text: string,
  defaultFormat: ImportDocumentFormat,
  namespace?: string
): ImportTarget[] {
  return parseLines(text).map((line, index) => {
    const parts = line.split(/\s+/);
    const url = parts[0] ?? "";
    const maybeFormat = parts[1];
    const format = isImportFormat(maybeFormat) ? maybeFormat : defaultFormat;
    const nameParts = isImportFormat(maybeFormat)
      ? parts.slice(2)
      : parts.slice(1);
    return {
      id: `document-url:${index}:${format}:${url}`,
      title: nameParts.join(" ") || shortUrlLabel(url),
      description: "Imported from a remote SBOL document.",
      url,
      format,
      namespace,
    };
  });
}

function toRemotePayload(target: ImportTarget): ImportRemoteDocumentPayload {
  return {
    url: target.url,
    format: target.format,
    namespace: target.namespace,
    name: target.title,
    description: target.description,
    created_by: "sbol-db-ui",
  };
}

function parseLines(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0 && !line.startsWith("#"));
}

function isImportFormat(
  value: string | undefined
): value is ImportDocumentFormat {
  return (
    value === "turtle" ||
    value === "jsonld" ||
    value === "rdfxml" ||
    value === "ntriples" ||
    value === "genbank" ||
    value === "fasta"
  );
}

function shortUrlLabel(url: string): string {
  try {
    const parsed = new URL(url);
    return (
      parsed.pathname.split("/").filter(Boolean).slice(-2).join("/") ||
      parsed.host
    );
  } catch {
    return url;
  }
}
