/**
 * Bounded graph neighborhood viewer. Walks outward from an IRI under
 * explicit depth, direction, predicate-allowlist, and node-cap bounds.
 *
 * Two views:
 *  - Table: nodes + edges as `DataTable`s, plus a status pill showing
 *    `max_depth_reached` and the truncation flag.
 *  - RDF: read-only Turtle / JSON-LD / RDF/XML / N-Triples dump from
 *    `/objects/neighborhood.rdf`, downloadable with the chosen extension.
 *
 * The URL is the source of truth for every control so a result is
 * shareable as a link. `useEffect`s are deliberately avoided; controls
 * write into the search params and the hooks react.
 */

import { useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { Download, GitBranch, Loader2 } from "lucide-react";

import { DataTable, type DataTableColumn } from "@/components/lab/DataTable";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { useNeighborhood, useNeighborhoodRdf } from "@/hooks/useObjects";
import {
  SERIALIZATION_FORMATS,
  serializationLabel,
  type NeighborhoodDirection,
  type NeighborhoodEdge,
  type NeighborhoodNode,
  type SerializationFormat,
} from "@/lib/api";

const DIRECTIONS: NeighborhoodDirection[] = ["forward", "backward", "both"];
const FORMAT_EXTENSION: Record<SerializationFormat, string> = {
  turtle: "ttl",
  jsonld: "jsonld",
  rdfxml: "rdf",
  ntriples: "nt",
};

export default function NeighborhoodRoute() {
  const [searchParams, setSearchParams] = useSearchParams();
  const [view, setView] = useState<"table" | "rdf">("table");
  const [rdfFormat, setRdfFormat] = useState<SerializationFormat>("turtle");

  const iri = searchParams.get("iri") ?? "";
  const depth = parseIntOr(searchParams.get("depth"), 2);
  const direction = (searchParams.get("direction") ??
    "forward") as NeighborhoodDirection;
  const literals = searchParams.get("literals") === "true";
  const maxNodes = parseIntOr(searchParams.get("max_nodes"), 2048);
  const predicates = useMemo(() => {
    const raw = searchParams.get("predicates");
    if (!raw) return [];
    return raw
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
  }, [searchParams]);

  const update = (patch: Record<string, string | null>) => {
    const next = new URLSearchParams(searchParams);
    for (const [k, v] of Object.entries(patch)) {
      if (v === null || v === "") next.delete(k);
      else next.set(k, v);
    }
    setSearchParams(next, { replace: true });
  };

  const query = {
    iri,
    depth,
    direction,
    predicates,
    max_nodes: maxNodes,
    literals,
  };

  const tableQuery = useNeighborhood(query, view === "table");
  const rdfQuery = useNeighborhoodRdf(query, rdfFormat, view === "rdf");

  const onDownloadRdf = async () => {
    if (!rdfQuery.data) return;
    const blob = new Blob([rdfQuery.data], {
      type: "text/plain;charset=utf-8",
    });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    const stem = iri.match(/[#/]([^#/]+)$/)?.[1] ?? "neighborhood";
    a.download = `${stem}-neighborhood.${FORMAT_EXTENSION[rdfFormat]}`;
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-6 px-8 py-10">
        <header>
          <div className="flex items-center gap-2">
            <GitBranch size={16} className="text-muted-foreground/70" />
            <h1 className="text-2xl font-semibold tracking-tight">
              Graph neighborhood
            </h1>
          </div>
          <p className="mt-2 text-sm text-muted-foreground">
            Bounded recursive traversal of{" "}
            <code className="font-mono">sbol_quads</code> from a root IRI.
            Depth, direction, and predicate allowlist are applied server-side;
            the URL preserves every control so the result is shareable.
          </p>
        </header>

        <section className="rounded-lg border bg-card">
          <div className="grid gap-3 border-b px-4 py-3 sm:grid-cols-2">
            <Field label="Root IRI">
              <input
                type="text"
                value={iri}
                placeholder="http://example.com/component/foo"
                onChange={(e) => update({ iri: e.target.value })}
                className="w-full rounded-md border bg-background px-3 py-1.5 font-mono text-[11px] text-foreground outline-none placeholder:text-muted-foreground/60 focus:ring-1 focus:ring-ring"
              />
            </Field>
            <Field label="Predicate allowlist (comma-separated IRIs)">
              <input
                type="text"
                value={searchParams.get("predicates") ?? ""}
                placeholder="empty = all predicates"
                onChange={(e) =>
                  update({
                    predicates: e.target.value.trim() || null,
                  })
                }
                className="w-full rounded-md border bg-background px-3 py-1.5 font-mono text-[11px] text-foreground outline-none placeholder:text-muted-foreground/60 focus:ring-1 focus:ring-ring"
              />
            </Field>
          </div>
          <div className="grid gap-3 px-4 py-3 sm:grid-cols-4">
            <Field label="Depth">
              <select
                value={String(depth)}
                onChange={(e) => update({ depth: e.target.value })}
                className="w-full rounded-md border bg-background px-2 py-1.5 text-xs text-foreground outline-none focus:ring-1 focus:ring-ring"
              >
                {[0, 1, 2, 3, 4, 5, 6].map((d) => (
                  <option key={d} value={d}>
                    {d}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Direction">
              <select
                value={direction}
                onChange={(e) => update({ direction: e.target.value })}
                className="w-full rounded-md border bg-background px-2 py-1.5 text-xs text-foreground outline-none focus:ring-1 focus:ring-ring"
              >
                {DIRECTIONS.map((d) => (
                  <option key={d} value={d}>
                    {d}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Max nodes">
              <input
                type="number"
                min={1}
                value={maxNodes}
                onChange={(e) => update({ max_nodes: e.target.value })}
                className="w-full rounded-md border bg-background px-2 py-1.5 text-xs text-foreground outline-none focus:ring-1 focus:ring-ring"
              />
            </Field>
            <Field label="Literals">
              <label className="flex h-7 items-center gap-2 text-xs text-foreground">
                <input
                  type="checkbox"
                  checked={literals}
                  onChange={(e) =>
                    update({
                      literals: e.target.checked ? "true" : null,
                    })
                  }
                />
                Include literal edges
              </label>
            </Field>
          </div>
        </section>

        {!iri ? (
          <div className="rounded-lg border bg-card px-6 py-10 text-center text-sm text-muted-foreground">
            Enter a root IRI to begin a traversal.
          </div>
        ) : (
          <>
            <div className="flex items-center gap-1 border-b">
              <ViewTab
                active={view === "table"}
                onClick={() => setView("table")}
              >
                Table
              </ViewTab>
              <ViewTab active={view === "rdf"} onClick={() => setView("rdf")}>
                RDF
              </ViewTab>
              <div className="ml-auto pb-2 text-xs text-muted-foreground">
                {tableQuery.data && view === "table" && (
                  <StatusPill
                    nodes={tableQuery.data.nodes.length}
                    edges={tableQuery.data.edges.length}
                    maxDepth={tableQuery.data.max_depth_reached}
                    truncated={tableQuery.data.truncated}
                  />
                )}
              </div>
            </div>

            {view === "table" ? (
              <TableView
                loading={tableQuery.isLoading || tableQuery.isFetching}
                error={tableQuery.error}
                nodes={tableQuery.data?.nodes ?? []}
                edges={tableQuery.data?.edges ?? []}
              />
            ) : (
              <RdfView
                loading={rdfQuery.isLoading || rdfQuery.isFetching}
                error={rdfQuery.error}
                body={rdfQuery.data ?? ""}
                format={rdfFormat}
                onFormatChange={setRdfFormat}
                onDownload={onDownloadRdf}
              />
            )}
          </>
        )}
      </div>
    </div>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <label className="block">
      <span className="mb-1 block text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
        {label}
      </span>
      {children}
    </label>
  );
}

function ViewTab({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`-mb-px border-b-2 px-3 py-1.5 text-xs font-medium transition-colors ${
        active
          ? "border-primary text-foreground"
          : "border-transparent text-muted-foreground hover:text-foreground"
      }`}
    >
      {children}
    </button>
  );
}

function StatusPill({
  nodes,
  edges,
  maxDepth,
  truncated,
}: {
  nodes: number;
  edges: number;
  maxDepth: number;
  truncated: boolean;
}) {
  return (
    <span className="inline-flex items-center gap-3 text-[11px] tabular-nums">
      <span>
        <strong className="text-foreground">{nodes.toLocaleString()}</strong>{" "}
        nodes
      </span>
      <span>
        <strong className="text-foreground">{edges.toLocaleString()}</strong>{" "}
        edges
      </span>
      <span>
        max depth <strong className="text-foreground">{maxDepth}</strong>
      </span>
      {truncated && (
        <span className="rounded-sm bg-amber-500/15 px-1.5 py-0.5 text-[10px] uppercase tracking-wider text-amber-500">
          truncated
        </span>
      )}
    </span>
  );
}

function TableView({
  loading,
  error,
  nodes,
  edges,
}: {
  loading: boolean;
  error: unknown;
  nodes: NeighborhoodNode[];
  edges: NeighborhoodEdge[];
}) {
  const nodeColumns: DataTableColumn<NeighborhoodNode>[] = [
    {
      id: "id",
      header: "ID",
      width: 320,
      cell: (n) => (
        <span className="truncate font-mono text-[11px] text-foreground">
          {n.id}
        </span>
      ),
      sortValue: (n) => n.id,
      filterValue: (n) => n.id,
    },
    {
      id: "depth",
      header: "Depth",
      width: 70,
      align: "right",
      cell: (n) => n.depth,
      sortValue: (n) => n.depth,
    },
    {
      id: "class",
      header: "Class",
      width: 180,
      cell: (n) => (
        <span className="font-mono text-[11px] text-muted-foreground">
          {shortIri(n.sbol_class ?? null)}
        </span>
      ),
      sortValue: (n) => n.sbol_class ?? "",
      filterValue: (n) => n.sbol_class ?? "",
    },
    {
      id: "name",
      header: "Name",
      width: 180,
      cell: (n) =>
        n.name ?? (
          <span className="text-muted-foreground/60">
            {n.blank_node ? "blank" : "—"}
          </span>
        ),
      sortValue: (n) => n.name ?? "",
    },
  ];

  const edgeColumns: DataTableColumn<NeighborhoodEdge>[] = [
    {
      id: "subject",
      header: "Subject",
      width: 280,
      cell: (e) => (
        <span className="truncate font-mono text-[11px] text-foreground">
          {e.subject}
        </span>
      ),
      sortValue: (e) => e.subject,
      filterValue: (e) => e.subject,
    },
    {
      id: "predicate",
      header: "Predicate",
      width: 200,
      cell: (e) => (
        <span className="truncate font-mono text-[11px] text-muted-foreground">
          {shortIri(e.predicate)}
        </span>
      ),
      sortValue: (e) => e.predicate,
      filterValue: (e) => e.predicate,
    },
    {
      id: "object",
      header: "Object",
      width: 320,
      cell: (e) => <ObjectCell o={e.object} />,
      sortValue: (e) => objectKey(e.object),
      filterValue: (e) => objectKey(e.object),
    },
    {
      id: "depth",
      header: "Depth",
      width: 70,
      align: "right",
      cell: (e) => e.depth,
      sortValue: (e) => e.depth,
    },
  ];

  if (error) {
    return (
      <ErrorBanner
        title="Couldn't fetch neighborhood"
        body={(error as Error).message}
      />
    );
  }
  if (loading && nodes.length === 0 && edges.length === 0) {
    return (
      <div className="flex items-center gap-2 rounded-lg border bg-card px-4 py-3 text-sm text-muted-foreground">
        <Loader2 size={14} className="animate-spin" />
        Walking the graph…
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <section>
        <SectionLabel>
          Nodes
          <span className="ml-2 tabular-nums text-muted-foreground">
            {nodes.length.toLocaleString()}
          </span>
        </SectionLabel>
        <div className="overflow-hidden rounded-lg border bg-card">
          <DataTable
            columns={nodeColumns}
            rows={nodes}
            rowKey={(n) => n.id}
            filterable
          />
        </div>
      </section>
      <section>
        <SectionLabel>
          Edges
          <span className="ml-2 tabular-nums text-muted-foreground">
            {edges.length.toLocaleString()}
          </span>
        </SectionLabel>
        <div className="overflow-hidden rounded-lg border bg-card">
          <DataTable
            columns={edgeColumns}
            rows={edges}
            rowKey={(e) =>
              `${e.subject}|${e.predicate}|${e.depth}|${objectKey(e.object)}`
            }
            filterable
          />
        </div>
      </section>
    </div>
  );
}

function RdfView({
  loading,
  error,
  body,
  format,
  onFormatChange,
  onDownload,
}: {
  loading: boolean;
  error: unknown;
  body: string;
  format: SerializationFormat;
  onFormatChange: (f: SerializationFormat) => void;
  onDownload: () => void;
}) {
  return (
    <section className="space-y-3">
      <div className="flex items-center gap-2">
        <label className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
          Format
        </label>
        <select
          value={format}
          onChange={(e) =>
            onFormatChange(e.target.value as SerializationFormat)
          }
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
          disabled={!body || loading}
          className="ml-auto inline-flex items-center gap-1.5 rounded-md border bg-background px-2.5 py-1 text-xs font-medium transition-colors hover:bg-accent/40 disabled:opacity-50"
        >
          <Download size={12} />
          Download
        </button>
      </div>

      {error ? (
        <ErrorBanner
          title="Couldn't fetch RDF subgraph"
          body={(error as Error).message}
        />
      ) : loading && !body ? (
        <div className="flex items-center gap-2 rounded-lg border bg-card px-4 py-3 text-sm text-muted-foreground">
          <Loader2 size={14} className="animate-spin" />
          Serializing…
        </div>
      ) : (
        <pre className="max-h-[60vh] overflow-auto rounded-lg border bg-card px-4 py-3 font-mono text-[11px] text-foreground">
          {body}
        </pre>
      )}
    </section>
  );
}

function ObjectCell({ o }: { o: NeighborhoodEdge["object"] }) {
  if ("iri" in o) {
    return (
      <span className="truncate font-mono text-[11px] text-foreground">
        {o.iri}
      </span>
    );
  }
  if ("blank" in o) {
    return (
      <span className="font-mono text-[11px] text-muted-foreground">
        _:{o.blank}
      </span>
    );
  }
  return (
    <span className="truncate text-[11px]">
      <span className="text-foreground">"{o.literal}"</span>
      <span className="ml-1 text-muted-foreground">
        ^^{shortIri(o.datatype)}
      </span>
    </span>
  );
}

function objectKey(o: NeighborhoodEdge["object"]): string {
  if ("iri" in o) return `iri:${o.iri}`;
  if ("blank" in o) return `blank:${o.blank}`;
  return `lit:${o.literal}^^${o.datatype}`;
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <h2 className="mb-3 flex items-center text-[11px] font-medium uppercase tracking-wider text-muted-foreground">
      {children}
    </h2>
  );
}

function shortIri(iri: string | null | undefined): string {
  if (!iri) return "";
  const m = iri.match(/[#/]([^#/]+)$/);
  return m ? m[1] : iri;
}

function parseIntOr(v: string | null, dflt: number): number {
  if (v === null) return dflt;
  const n = parseInt(v, 10);
  return Number.isFinite(n) ? n : dflt;
}
