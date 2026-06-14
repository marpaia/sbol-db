import { useMemo, useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  Check,
  Database,
  DownloadCloud,
  ExternalLink,
  FileArchive,
  Globe2,
  Layers3,
  Loader2,
  TriangleAlert,
} from "lucide-react";
import { useNavigate } from "react-router-dom";

import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  enqueueRemoteImport,
  enqueueSynBioHubCollection,
  importFormatLabel,
  type EnqueueJobResult,
  type ImportDocumentFormat,
  type ImportRemoteDocumentPayload,
  type ImportSynBioHubCollectionPayload,
} from "@/lib/api";
import { describeError } from "@/lib/utils";

const SYNBIOHUB_BASE_URL = "https://synbiohub.org";
const SYNBIOHUB_IGEM_NAMESPACE = "https://synbiohub.org/public/igem";

type DatasetTarget = {
  id: string;
  importKind: "remote_document" | "synbiohub_collection";
  title: string;
  provider: string;
  description: string;
  url: string;
  baseUrl?: string;
  pageSize?: number;
  format: ImportDocumentFormat;
  namespace?: string;
  sizeLabel: string;
  icon: ReactNode;
};

type DatasetState =
  | { kind: "idle" }
  | { kind: "queued"; jobId: string; deduplicated: boolean }
  | { kind: "failed"; message: string };

const DATASET_PRESETS: DatasetTarget[] = [
  {
    id: "synbiohub-igem",
    importKind: "synbiohub_collection",
    title: "iGEM Registry",
    provider: "SynBioHub public/igem",
    description:
      "Whole public iGEM collection. The mirror job discovers members through SynBioHub SPARQL and imports each SBOL 2 component document.",
    url: "https://synbiohub.org/public/igem/igem_collection/1",
    baseUrl: SYNBIOHUB_BASE_URL,
    pageSize: 250,
    format: "rdfxml",
    namespace: SYNBIOHUB_IGEM_NAMESPACE,
    sizeLabel: "whole collection mirror",
    icon: <Layers3 size={18} />,
  },
  {
    id: "synbiohub-bsu",
    importKind: "synbiohub_collection",
    title: "Bacillus subtilis",
    provider: "SynBioHub public/bsu",
    description:
      "Whole SynBioHub collection for Bacillus subtilis designs and regulatory parts.",
    url: "https://synbiohub.org/public/bsu/bsu_collection/1",
    baseUrl: SYNBIOHUB_BASE_URL,
    pageSize: 250,
    format: "rdfxml",
    namespace: "https://synbiohub.org/public/bsu",
    sizeLabel: "whole collection mirror",
    icon: <Database size={18} />,
  },
  {
    id: "synbiohub-eco1c1g1t1",
    importKind: "synbiohub_collection",
    title: "Eco1C1G1T1",
    provider: "SynBioHub public/Eco1C1G1T1",
    description:
      "Published SynBioHub design collection mirrored as per-component SBOL imports.",
    url: "https://synbiohub.org/public/Eco1C1G1T1/Eco1C1G1T1_collection/1",
    baseUrl: SYNBIOHUB_BASE_URL,
    pageSize: 250,
    format: "rdfxml",
    namespace: "https://synbiohub.org/public/Eco1C1G1T1",
    sizeLabel: "whole collection mirror",
    icon: <FileArchive size={18} />,
  },
];

export default function ImportRoute() {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [states, setStates] = useState<Record<string, DatasetState>>({});
  const [submitting, setSubmitting] = useState(false);
  const [pageError, setPageError] = useState<string | null>(null);
  const [lastJob, setLastJob] = useState<EnqueueJobResult | null>(null);

  const [collectionUri, setCollectionUri] = useState(
    "https://synbiohub.org/public/igem/igem_collection/1"
  );
  const [collectionName, setCollectionName] = useState("iGEM Registry");
  const [collectionNamespace, setCollectionNamespace] = useState(
    SYNBIOHUB_IGEM_NAMESPACE
  );

  const [urlList, setUrlList] = useState("");
  const [urlFormat, setUrlFormat] = useState<ImportDocumentFormat>("rdfxml");
  const [urlNamespace, setUrlNamespace] = useState("");

  const selectedTargets = useMemo(
    () => DATASET_PRESETS.filter((target) => selected.has(target.id)),
    [selected]
  );
  const queuedCount = Object.values(states).filter(
    (state) => state.kind === "queued"
  ).length;
  const failedCount = Object.values(states).filter(
    (state) => state.kind === "failed"
  ).length;

  const customCollectionTarget = useMemo<DatasetTarget | null>(() => {
    const uri = collectionUri.trim();
    if (!uri) return null;
    return {
      id: `synbiohub-collection:${uri}`,
      importKind: "synbiohub_collection",
      title: collectionName.trim() || shortUrlLabel(uri),
      provider: providerFromUrl(uri) ?? "SynBioHub",
      description: "Whole SynBioHub collection mirror.",
      url: stripSynBioHubDownloadSuffix(uri),
      baseUrl: SYNBIOHUB_BASE_URL,
      pageSize: 250,
      format: "rdfxml",
      namespace: collectionNamespace.trim() || undefined,
      sizeLabel: "whole collection mirror",
      icon: <Layers3 size={18} />,
    };
  }, [collectionName, collectionNamespace, collectionUri]);

  const urlTargets = useMemo(
    () => parseUrlTargets(urlList, urlFormat, urlNamespace.trim() || undefined),
    [urlList, urlFormat, urlNamespace]
  );

  const enqueueTargets = async (targets: DatasetTarget[]) => {
    if (targets.length === 0) return;
    setSubmitting(true);
    setPageError(null);
    let newest: EnqueueJobResult | null = null;
    for (const target of targets) {
      try {
        const result =
          target.importKind === "synbiohub_collection"
            ? await enqueueSynBioHubCollection(toSynBioHubPayload(target), {
                max_attempts: 3,
                idempotency_key: `synbiohub-collection:${target.format}:${target.url}`,
              })
            : await enqueueRemoteImport(toRemotePayload(target), {
                max_attempts: 3,
                idempotency_key: `dataset-import:${target.format}:${target.url}`,
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
              Queue whole public datasets for SBOL 3 ingest.
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            {lastJob && (
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() =>
                  navigate(`/observability/jobs/${lastJob.job.id}`)
                }
              >
                <ExternalLink size={14} />
                Latest job
              </Button>
            )}
            <Button
              type="button"
              size="sm"
              disabled={submitting || selectedTargets.length === 0}
              onClick={() => enqueueTargets(selectedTargets)}
            >
              {submitting ? (
                <Loader2 size={14} className="animate-spin" />
              ) : (
                <DownloadCloud size={14} />
              )}
              Enqueue selected datasets
            </Button>
          </div>
        </header>

        {pageError && (
          <ErrorBanner title="Import enqueue failed" body={pageError} />
        )}

        <section className="grid gap-3 sm:grid-cols-3">
          <MetricCard
            label="Selected datasets"
            value={selectedTargets.length}
          />
          <MetricCard label="Queued jobs" value={queuedCount} />
          <MetricCard label="Failed enqueues" value={failedCount} />
        </section>

        <Tabs defaultValue="datasets">
          <TabsList className="h-auto flex-wrap justify-start">
            <TabsTrigger value="datasets">Public datasets</TabsTrigger>
            <TabsTrigger value="collection">SynBioHub collection</TabsTrigger>
            <TabsTrigger value="urls">Dataset URLs</TabsTrigger>
          </TabsList>

          <TabsContent value="datasets" className="space-y-4">
            <section className="flex flex-wrap items-center gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() =>
                  setSelected(
                    new Set(DATASET_PRESETS.map((target) => target.id))
                  )
                }
              >
                Select all datasets
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => setSelected(new Set())}
              >
                Clear
              </Button>
            </section>

            <section className="grid gap-3 lg:grid-cols-3">
              {DATASET_PRESETS.map((target) => (
                <DatasetCard
                  key={target.id}
                  target={target}
                  selected={selected.has(target.id)}
                  state={states[target.id]}
                  submitting={submitting}
                  onToggle={() =>
                    setSelected((prev) => {
                      const next = new Set(prev);
                      if (next.has(target.id)) next.delete(target.id);
                      else next.add(target.id);
                      return next;
                    })
                  }
                  onEnqueue={() => enqueueTargets([target])}
                  onOpenJob={(jobId) =>
                    navigate(`/observability/jobs/${jobId}`)
                  }
                />
              ))}
            </section>
          </TabsContent>

          <TabsContent value="collection">
            <CollectionPanel
              uri={collectionUri}
              onUri={setCollectionUri}
              name={collectionName}
              onName={setCollectionName}
              namespace={collectionNamespace}
              onNamespace={setCollectionNamespace}
              target={customCollectionTarget}
              state={
                customCollectionTarget
                  ? states[customCollectionTarget.id]
                  : undefined
              }
              submitting={submitting}
              onEnqueue={() =>
                customCollectionTarget
                  ? enqueueTargets([customCollectionTarget])
                  : undefined
              }
              onOpenJob={(jobId) => navigate(`/observability/jobs/${jobId}`)}
            />
          </TabsContent>

          <TabsContent value="urls">
            <UrlListPanel
              value={urlList}
              onChange={setUrlList}
              format={urlFormat}
              onFormat={setUrlFormat}
              namespace={urlNamespace}
              onNamespace={setUrlNamespace}
              targets={urlTargets}
              states={states}
              submitting={submitting}
              onEnqueue={() => enqueueTargets(urlTargets)}
              onOpenJob={(jobId) => navigate(`/observability/jobs/${jobId}`)}
            />
          </TabsContent>
        </Tabs>
      </div>
    </div>
  );
}

function DatasetCard({
  target,
  selected,
  state,
  submitting,
  onToggle,
  onEnqueue,
  onOpenJob,
}: {
  target: DatasetTarget;
  selected?: boolean;
  state?: DatasetState;
  submitting: boolean;
  onToggle?: () => void;
  onEnqueue: () => void;
  onOpenJob: (jobId: string) => void;
}) {
  return (
    <article className="flex min-h-80 flex-col rounded-lg border bg-card p-4">
      <div className="flex items-start gap-3">
        {onToggle && (
          <input
            type="checkbox"
            checked={!!selected}
            onChange={onToggle}
            aria-label={`Select ${target.title}`}
            className="mt-1 h-4 w-4 rounded border-muted-foreground/40"
          />
        )}
        <div className="rounded-md border bg-background p-2 text-muted-foreground">
          {target.icon}
        </div>
        <div className="min-w-0">
          <h2 className="text-sm font-medium">{target.title}</h2>
          <p className="mt-0.5 text-xs text-muted-foreground">
            {target.provider}
          </p>
        </div>
      </div>

      <p className="mt-4 text-sm leading-6 text-muted-foreground">
        {target.description}
      </p>

      <dl className="mt-4 grid gap-2 text-xs">
        <MetaRow label="Scope" value={target.sizeLabel} />
        <MetaRow label="Format" value={importFormatLabel(target.format)} />
        <MetaRow
          label="Mode"
          value={
            target.importKind === "synbiohub_collection"
              ? "SynBioHub mirror"
              : "Remote document"
          }
        />
      </dl>

      <div className="mt-4 truncate rounded-md border bg-background px-2 py-1.5 font-mono text-[11px] text-muted-foreground">
        {target.url}
      </div>

      <div className="mt-auto flex items-center justify-between gap-3 pt-4">
        <StatusPill state={state} onOpenJob={onOpenJob} />
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={submitting}
          onClick={onEnqueue}
        >
          {submitting ? (
            <Loader2 size={14} className="animate-spin" />
          ) : (
            <DownloadCloud size={14} />
          )}
          Import dataset
        </Button>
      </div>
    </article>
  );
}

function CollectionPanel({
  uri,
  onUri,
  name,
  onName,
  namespace,
  onNamespace,
  target,
  state,
  submitting,
  onEnqueue,
  onOpenJob,
}: {
  uri: string;
  onUri: (value: string) => void;
  name: string;
  onName: (value: string) => void;
  namespace: string;
  onNamespace: (value: string) => void;
  target: DatasetTarget | null;
  state?: DatasetState;
  submitting: boolean;
  onEnqueue: () => void;
  onOpenJob: (jobId: string) => void;
}) {
  return (
    <section className="grid gap-4 lg:grid-cols-[minmax(0,1fr)_360px]">
      <div className="rounded-lg border bg-card p-4">
        <div className="flex items-center gap-2">
          <Globe2 size={16} className="text-muted-foreground" />
          <h2 className="text-sm font-medium">SynBioHub collection</h2>
        </div>
        <div className="mt-4 grid gap-3">
          <TextField
            label="Collection URI"
            value={uri}
            onChange={onUri}
            placeholder="https://synbiohub.org/public/igem/igem_collection/1"
          />
          <TextField
            label="Name"
            value={name}
            onChange={onName}
            placeholder="Dataset name"
          />
          <TextField
            label="Namespace"
            value={namespace}
            onChange={onNamespace}
            placeholder="https://synbiohub.org/public/igem"
            mono
          />
        </div>
      </div>

      {target && (
        <DatasetCard
          target={target}
          state={state}
          submitting={submitting}
          onEnqueue={onEnqueue}
          onOpenJob={onOpenJob}
        />
      )}
    </section>
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
  targets: DatasetTarget[];
  states: Record<string, DatasetState>;
  submitting: boolean;
  onEnqueue: () => void;
  onOpenJob: (jobId: string) => void;
}) {
  return (
    <section className="rounded-lg border bg-card">
      <div className="flex flex-wrap items-center gap-3 border-b px-4 py-3">
        <div className="flex items-center gap-2">
          <Globe2 size={16} className="text-muted-foreground" />
          <h2 className="text-sm font-medium">Dataset URLs</h2>
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
            onChange={(e) => onChange(e.target.value)}
            placeholder="https://synbiohub.org/public/igem/igem_collection/1/sbol"
            className="min-h-64 resize-y rounded-md border bg-background p-3 font-mono text-xs text-foreground outline-none focus:ring-1 focus:ring-ring"
          />
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
  state?: DatasetState;
  onOpenJob: (jobId: string) => void;
}) {
  if (!state || state.kind === "idle") {
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

function MetaRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="grid grid-cols-[72px_minmax(0,1fr)] gap-2">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="truncate text-foreground">{value}</dd>
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
        onChange={(e) => onChange(e.target.value)}
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
        onChange={(e) => onChange(e.target.value as ImportDocumentFormat)}
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
): DatasetTarget[] {
  return parseLines(text).map((line, index) => {
    const parts = line.split(/\s+/);
    const url = parts[0] ?? "";
    const maybeFormat = parts[1];
    const format = isImportFormat(maybeFormat) ? maybeFormat : defaultFormat;
    const nameParts = isImportFormat(maybeFormat)
      ? parts.slice(2)
      : parts.slice(1);
    return {
      id: `dataset-url:${index}:${format}:${url}`,
      importKind: "remote_document",
      title: nameParts.join(" ") || shortUrlLabel(url),
      provider: providerFromUrl(url) ?? "Remote URL",
      description: "Remote dataset download.",
      url,
      format,
      namespace,
      sizeLabel: "dataset URL",
      icon: <Globe2 size={18} />,
    };
  });
}

function toRemotePayload(target: DatasetTarget): ImportRemoteDocumentPayload {
  return {
    url: target.url,
    format: target.format,
    namespace: target.namespace,
    name: target.title,
    description: target.description,
    created_by: "sbol-db-ui",
  };
}

function toSynBioHubPayload(
  target: DatasetTarget
): ImportSynBioHubCollectionPayload {
  return {
    collection_uri: target.url,
    base_url: target.baseUrl,
    format: target.format,
    namespace: target.namespace,
    page_size: target.pageSize,
    created_by: "sbol-db-ui",
  };
}

function parseLines(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0 && !line.startsWith("#"));
}

function stripSynBioHubDownloadSuffix(uri: string): string {
  const clean = uri.replace(/\/+$/, "");
  return clean.replace(/\/(sbol|sbolnr|gb|fasta)$/, "");
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

function providerFromUrl(url: string): string | null {
  try {
    return new URL(url).host;
  } catch {
    return null;
  }
}
