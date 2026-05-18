/**
 * Typed object detail. Resolves an IRI via `GET /objects` and surfaces
 * the typed record: class, display id, name, types, roles, version, and
 * the raw `data` JSON. Action buttons let the user re-emit the object
 * subgraph as RDF (Turtle / JSON-LD / RDF/XML / N-Triples) or jump to
 * the neighborhood traversal viewer pre-filled with this IRI.
 */

import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  ChevronLeft,
  Copy,
  Download,
  ExternalLink,
  GitBranch,
  Loader2,
  TriangleAlert,
} from "lucide-react";
import { Link, useNavigate, useParams } from "react-router-dom";

import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { useObjectByIri } from "@/hooks/useObjects";
import {
  ApiError,
  exportObjectRdf,
  SERIALIZATION_FORMATS,
  serializationLabel,
  type SbolObjectRecord,
  type SerializationFormat,
} from "@/lib/api";
import { describeError } from "@/lib/utils";

const FORMAT_EXTENSION: Record<SerializationFormat, string> = {
  turtle: "ttl",
  jsonld: "jsonld",
  rdfxml: "rdf",
  ntriples: "nt",
};

export default function ObjectDetailRoute() {
  const params = useParams<{ iri: string }>();
  const iri = decodeURIComponent(params.iri ?? "");
  const navigate = useNavigate();
  const { data, error, isLoading } = useObjectByIri(iri);

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-5xl space-y-6 px-8 py-10">
        <Link
          to="/objects"
          className="inline-flex items-center gap-1 text-xs text-muted-foreground transition-colors hover:text-foreground"
        >
          <ChevronLeft size={12} />
          Object browser
        </Link>

        {error instanceof ApiError && error.status === 404 ? (
          <NotFound iri={iri} />
        ) : error ? (
          <ErrorBanner
            title="Couldn't load object"
            body={(error as Error).message}
          />
        ) : isLoading || !data ? (
          <Skeleton />
        ) : (
          <>
            <Header object={data} />
            <Actions
              object={data}
              onNeighborhood={() =>
                navigate(`/neighborhood?iri=${encodeURIComponent(data.iri)}`)
              }
            />
            <Properties object={data} />
            <RawData object={data} />
          </>
        )}
      </div>
    </div>
  );
}

function Header({ object }: { object: SbolObjectRecord }) {
  const queryClient = useQueryClient();
  return (
    <header className="space-y-1.5">
      <div className="flex items-center gap-2">
        <h1 className="text-2xl font-semibold tracking-tight">
          {object.name ?? object.display_id ?? "Untitled object"}
        </h1>
        <button
          type="button"
          onClick={() => {
            void navigator.clipboard.writeText(object.iri);
            queryClient.setQueryData(["lab", "copied-toast"], Date.now());
          }}
          className="text-muted-foreground transition-colors hover:text-foreground"
          aria-label="Copy IRI"
          title="Copy IRI"
        >
          <Copy size={14} />
        </button>
      </div>
      <div className="truncate font-mono text-[11px] text-muted-foreground/80">
        {object.iri}
      </div>
      {object.sbol_class && (
        <div className="font-mono text-[11px] text-muted-foreground">
          a <span className="text-foreground">{object.sbol_class}</span>
        </div>
      )}
    </header>
  );
}

function Actions({
  object,
  onNeighborhood,
}: {
  object: SbolObjectRecord;
  onNeighborhood: () => void;
}) {
  const [format, setFormat] = useState<SerializationFormat>("turtle");
  const [phase, setPhase] = useState<
    "idle" | "loading" | { kind: "error"; msg: string }
  >("idle");

  const onDownload = async () => {
    setPhase("loading");
    try {
      const text = await exportObjectRdf(object.id, format);
      const blob = new Blob([text], { type: "text/plain;charset=utf-8" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      const base = (object.display_id ?? object.id).replace(/\s+/g, "-");
      a.download = `${base}.${FORMAT_EXTENSION[format]}`;
      a.click();
      URL.revokeObjectURL(url);
      setPhase("idle");
    } catch (err) {
      setPhase({ kind: "error", msg: describeError(err) });
    }
  };

  const isHttp = /^https?:\/\//i.test(object.iri);

  return (
    <section className="flex flex-wrap items-center gap-2 rounded-lg border bg-card px-4 py-3">
      <button
        type="button"
        onClick={onNeighborhood}
        className="inline-flex items-center gap-1.5 rounded-md bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90"
      >
        <GitBranch size={14} />
        Walk neighborhood
      </button>

      <div className="ml-2 flex items-center gap-1.5">
        <label className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
          Export
        </label>
        <select
          value={format}
          onChange={(e) => setFormat(e.target.value as SerializationFormat)}
          className="rounded-md border bg-background px-2 py-1 text-xs text-foreground outline-none focus:ring-1 focus:ring-ring"
        >
          {SERIALIZATION_FORMATS.map((f) => (
            <option key={f} value={f}>
              {serializationLabel(f)}
            </option>
          ))}
        </select>
        <button
          type="button"
          onClick={onDownload}
          disabled={phase === "loading"}
          className="inline-flex items-center gap-1.5 rounded-md border bg-background px-2.5 py-1 text-xs font-medium transition-colors hover:bg-accent/40 disabled:opacity-50"
        >
          {phase === "loading" ? (
            <Loader2 size={12} className="animate-spin" />
          ) : (
            <Download size={12} />
          )}
          Download
        </button>
      </div>

      {isHttp && (
        <a
          href={object.iri}
          target="_blank"
          rel="noopener noreferrer"
          className="ml-auto inline-flex items-center gap-1.5 text-xs text-muted-foreground transition-colors hover:text-foreground"
        >
          <ExternalLink size={12} />
          Open IRI
        </a>
      )}

      {typeof phase === "object" && phase.kind === "error" && (
        <div className="w-full text-xs text-destructive">{phase.msg}</div>
      )}
    </section>
  );
}

function Properties({ object }: { object: SbolObjectRecord }) {
  return (
    <section className="rounded-lg border bg-card px-4 py-3">
      <dl className="grid gap-3 text-sm sm:grid-cols-2">
        <Pair label="Display ID" value={object.display_id} />
        <Pair label="Version" value={object.version} />
        <Pair
          label="Persistent identity"
          value={object.persistent_identity}
          mono
        />
        <Pair label="Created" value={object.created_at ?? null} />
        <PairList label="Types" values={object.types ?? []} />
        <PairList label="Roles" values={object.roles ?? []} />
      </dl>
    </section>
  );
}

function Pair({
  label,
  value,
  mono,
}: {
  label: string;
  value: string | null | undefined;
  mono?: boolean;
}) {
  return (
    <div>
      <dt className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
        {label}
      </dt>
      <dd
        className={`mt-0.5 truncate text-foreground ${
          mono ? "font-mono text-[11px]" : "text-sm"
        }`}
      >
        {value || <span className="text-muted-foreground/60">—</span>}
      </dd>
    </div>
  );
}

function PairList({ label, values }: { label: string; values: string[] }) {
  return (
    <div>
      <dt className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
        {label} ({values.length})
      </dt>
      <dd className="mt-1 flex flex-wrap gap-1.5">
        {values.length === 0 ? (
          <span className="text-sm text-muted-foreground/60">—</span>
        ) : (
          values.map((v) => (
            <span
              key={v}
              title={v}
              className="rounded-sm border bg-background px-1.5 py-0.5 font-mono text-[10px] text-foreground"
            >
              {shortIri(v)}
            </span>
          ))
        )}
      </dd>
    </div>
  );
}

function RawData({ object }: { object: SbolObjectRecord }) {
  if (!object.data) return null;
  const json = JSON.stringify(object.data, null, 2);
  return (
    <section>
      <SectionLabel>Raw projection</SectionLabel>
      <pre className="max-h-96 overflow-auto rounded-lg border bg-card px-4 py-3 font-mono text-[11px] text-foreground">
        {json}
      </pre>
    </section>
  );
}

function NotFound({ iri }: { iri: string }) {
  return (
    <div className="flex items-start gap-3 rounded-md border bg-muted/40 px-3 py-3 text-sm">
      <TriangleAlert
        size={14}
        className="mt-0.5 shrink-0 text-muted-foreground"
        aria-hidden
      />
      <div>
        <div className="font-medium text-foreground">Object not found</div>
        <div className="mt-0.5 text-muted-foreground">
          No object at <code className="font-mono">{iri}</code>.
        </div>
      </div>
    </div>
  );
}

function Skeleton() {
  return (
    <div className="space-y-3">
      <div className="h-12 animate-pulse rounded-md bg-card" />
      <div className="h-16 animate-pulse rounded-md bg-card" />
      <div className="h-48 animate-pulse rounded-md bg-card" />
    </div>
  );
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <h2 className="mb-3 text-[11px] font-medium uppercase tracking-wider text-muted-foreground">
      {children}
    </h2>
  );
}

function shortIri(iri: string): string {
  const m = iri.match(/[#/]([^#/]+)$/);
  return m ? m[1] : iri;
}
