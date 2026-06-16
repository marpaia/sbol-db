/**
 * Per-graph detail. Shows the graph's kind, triple/object counts, and
 * provenance. For an `sbol3` graph it also lists the SBOL objects derived
 * from its triples (object rows link to the typed object detail). A
 * `verbatim` graph has no derived object view; its content is the triples
 * themselves, browsed directly as a paginated triple table.
 */

import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChevronLeft, Share2, TriangleAlert } from "lucide-react";
import { Link, useNavigate, useParams } from "react-router-dom";

import { DataTable, type DataTableColumn } from "@/components/lab/DataTable";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { KpiTile } from "@/components/observability/KpiTile";
import { useGraph } from "@/hooks/useGraphs";
import {
  ApiError,
  listGraphTriples,
  listObjects,
  type GraphTriple,
  type GraphSummary,
  type GraphTerm,
  type SbolObjectRecord,
} from "@/lib/api";
import { formatRelative } from "@/lib/utils";

const PAGE_SIZE = 100;

export default function GraphDetailRoute() {
  const navigate = useNavigate();
  const params = useParams<{ id: string }>();
  const id = params.id ?? "";

  const { data, isLoading, error } = useGraph(id);

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-6 px-8 py-10">
        <Link
          to="/graphs"
          className="inline-flex items-center gap-1 text-xs text-muted-foreground transition-colors hover:text-foreground"
        >
          <ChevronLeft size={12} />
          All graphs
        </Link>

        {error instanceof ApiError && error.status === 404 ? (
          <NotFound id={id} />
        ) : error ? (
          <ErrorBanner
            title="Couldn't load graph"
            body={(error as Error).message}
          />
        ) : isLoading || !data ? (
          <Skeleton />
        ) : (
          <>
            <Header graph={data} />
            <div className="grid gap-3 sm:grid-cols-3">
              <KpiTile label="Triples" value={data.triple_count.toLocaleString()} />
              <KpiTile
                label="Objects"
                value={data.object_count.toLocaleString()}
              />
              <KpiTile
                label="Format"
                value={data.serialization_format ?? "—"}
              />
            </div>
            <Metadata graph={data} />
            {data.kind === "sbol3" ? (
              <section>
                <SectionLabel>Objects in this graph</SectionLabel>
                <ObjectsForGraph
                  graphId={data.id}
                  onOpen={(iri) =>
                    navigate(`/objects/${encodeURIComponent(iri)}`)
                  }
                />
              </section>
            ) : (
              <section>
                <SectionLabel>Triples in this graph</SectionLabel>
                <p className="mb-3 -mt-1 text-xs text-muted-foreground">
                  A <strong>verbatim</strong> graph is stored as written, with
                  no derived SBOL object view. Its content is the triples
                  themselves.
                </p>
                <TriplesForGraph graphId={data.id} />
              </section>
            )}
          </>
        )}
      </div>
    </div>
  );
}

function Header({ graph }: { graph: GraphSummary }) {
  return (
    <header className="space-y-1.5">
      <div className="flex items-center gap-2">
        <Share2 size={16} className="text-muted-foreground/70" aria-hidden />
        <h1 className="text-2xl font-semibold tracking-tight">
          {graph.name ?? "Untitled graph"}
        </h1>
      </div>
      <div className="truncate font-mono text-[11px] text-muted-foreground/80">
        {graph.iri}
      </div>
    </header>
  );
}

function Metadata({ graph }: { graph: GraphSummary }) {
  return (
    <section className="rounded-lg border bg-card px-4 py-3">
      <dl className="grid gap-3 text-sm sm:grid-cols-3">
        <Pair label="Kind" value={graph.kind} />
        <Pair label="Source URI" value={graph.source_uri} mono />
        <Pair
          label="Created"
          value={`${formatRelative(graph.created_at)} (${new Date(graph.created_at).toLocaleString()})`}
        />
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
  value: string | null;
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
        {value ?? <span className="text-muted-foreground/60">—</span>}
      </dd>
    </div>
  );
}

function ObjectsForGraph({
  graphId,
  onOpen,
}: {
  graphId: string;
  onOpen: (iri: string) => void;
}) {
  const [cursors, setCursors] = useState<string[]>([""]);
  const after = cursors[cursors.length - 1] || undefined;
  const page = cursors.length - 1;

  const { data, isLoading, error } = useQuery({
    queryKey: ["lab", "objects", "by-graph", graphId, after ?? ""],
    queryFn: ({ signal }) =>
      listObjects({ graph_id: graphId, limit: PAGE_SIZE, after }, signal),
    placeholderData: (prev) => prev,
  });

  const columns: DataTableColumn<SbolObjectRecord>[] = [
    {
      id: "display",
      header: "Display ID / IRI",
      width: 380,
      cell: (o) => (
        <div className="min-w-0">
          {o.display_id && (
            <div className="truncate text-foreground">{o.display_id}</div>
          )}
          <div className="truncate font-mono text-[11px] text-muted-foreground">
            {o.iri}
          </div>
        </div>
      ),
      sortValue: (o) => o.iri,
      filterValue: (o) => `${o.display_id ?? ""} ${o.iri} ${o.name ?? ""}`.trim(),
    },
    {
      id: "name",
      header: "Name",
      width: 200,
      cell: (o) => o.name ?? <span className="text-muted-foreground/60">—</span>,
      sortValue: (o) => o.name?.toLowerCase() ?? "",
    },
    {
      id: "class",
      header: "Class",
      width: 220,
      cell: (o) => (
        <span className="font-mono text-[11px] text-muted-foreground">
          {shortIri(o.sbol_class)}
        </span>
      ),
      sortValue: (o) => o.sbol_class ?? "",
      filterValue: (o) => o.sbol_class ?? "",
    },
  ];

  if (error) {
    return (
      <ErrorBanner
        title="Couldn't list objects"
        body={(error as Error).message}
      />
    );
  }
  if (isLoading && !data) return <Skeleton />;
  if (!data || data.objects.length === 0) {
    return (
      <div className="rounded-lg border bg-card px-4 py-6 text-sm text-muted-foreground">
        No objects projected from this graph yet.
      </div>
    );
  }

  return (
    <div className="space-y-3">
      <div className="overflow-hidden rounded-lg border bg-card">
        <DataTable
          columns={columns}
          rows={data.objects}
          rowKey={(o) => o.id}
          filterable
          onRowClick={(o) => onOpen(o.iri)}
        />
      </div>
      <div className="flex items-center justify-between gap-2 text-xs">
        <div className="text-muted-foreground">
          Page {page + 1}
          {!data.next_cursor && " · end"}
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => setCursors((prev) => prev.slice(0, -1))}
            disabled={page === 0}
            className="rounded-md border px-2.5 py-1 font-medium transition-colors hover:bg-accent/40 disabled:cursor-not-allowed disabled:opacity-50"
          >
            Previous
          </button>
          <button
            type="button"
            onClick={() =>
              data.next_cursor &&
              setCursors((prev) => [...prev, data.next_cursor!])
            }
            disabled={!data.next_cursor}
            className="rounded-md border px-2.5 py-1 font-medium transition-colors hover:bg-accent/40 disabled:cursor-not-allowed disabled:opacity-50"
          >
            Next
          </button>
        </div>
      </div>
    </div>
  );
}

const TRIPLES_PAGE_SIZE = 100;

function TriplesForGraph({ graphId }: { graphId: string }) {
  const [page, setPage] = useState(0);
  const offset = page * TRIPLES_PAGE_SIZE;

  const { data, isLoading, error } = useQuery({
    queryKey: ["lab", "graphs", graphId, "triples", offset],
    queryFn: ({ signal }) =>
      listGraphTriples(graphId, { limit: TRIPLES_PAGE_SIZE, offset }, signal),
    placeholderData: (prev) => prev,
  });

  const columns: DataTableColumn<GraphTriple>[] = [
    {
      id: "subject",
      header: "Subject",
      width: 300,
      cell: (q) => <TermCell term={q.subject} />,
      sortValue: (q) => q.subject.value,
      filterValue: (q) => q.subject.value,
    },
    {
      id: "predicate",
      header: "Predicate",
      width: 260,
      cell: (q) => <TermCell term={q.predicate} />,
      sortValue: (q) => q.predicate.value,
      filterValue: (q) => q.predicate.value,
    },
    {
      id: "object",
      header: "Object",
      width: 420,
      cell: (q) => <TermCell term={q.object} />,
      sortValue: (q) => q.object.value,
      filterValue: (q) => q.object.value,
    },
  ];

  if (error) {
    return (
      <ErrorBanner
        title="Couldn't load triples"
        body={(error as Error).message}
      />
    );
  }
  if (isLoading && !data) return <Skeleton />;
  if (!data || data.triples.length === 0) {
    return (
      <div className="rounded-lg border bg-card px-4 py-6 text-sm text-muted-foreground">
        This graph has no triples.
      </div>
    );
  }

  const totalPages = Math.max(1, Math.ceil(data.total / TRIPLES_PAGE_SIZE));

  return (
    <div className="space-y-3">
      <div className="overflow-hidden rounded-lg border bg-card">
        <DataTable
          columns={columns}
          rows={data.triples}
          rowKey={(q) => `${q.subject.value} ${q.predicate.value} ${q.object.value}`}
          filterable
        />
      </div>
      <div className="flex items-center justify-between gap-2 text-xs">
        <div className="text-muted-foreground">
          {data.total.toLocaleString()} triples · page {page + 1} of{" "}
          {totalPages}
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => setPage((p) => Math.max(0, p - 1))}
            disabled={page === 0}
            className="rounded-md border px-2.5 py-1 font-medium transition-colors hover:bg-accent/40 disabled:cursor-not-allowed disabled:opacity-50"
          >
            Previous
          </button>
          <button
            type="button"
            onClick={() => setPage((p) => Math.min(totalPages - 1, p + 1))}
            disabled={page >= totalPages - 1}
            className="rounded-md border px-2.5 py-1 font-medium transition-colors hover:bg-accent/40 disabled:cursor-not-allowed disabled:opacity-50"
          >
            Next
          </button>
        </div>
      </div>
    </div>
  );
}

function TermCell({ term }: { term: GraphTerm }) {
  if (term.type === "literal") {
    return (
      <div className="min-w-0">
        <div className="truncate text-foreground">{term.value || " "}</div>
        {(term.language || term.datatype) && (
          <div className="truncate font-mono text-[10px] text-muted-foreground/70">
            {term.language ? `@${term.language}` : shortIri(term.datatype)}
          </div>
        )}
      </div>
    );
  }
  if (term.type === "bnode") {
    return (
      <span className="font-mono text-[11px] text-muted-foreground">
        _:{term.value}
      </span>
    );
  }
  return (
    <div className="min-w-0" title={term.value}>
      <div className="truncate font-mono text-[11px] text-foreground">
        {term.value}
      </div>
    </div>
  );
}

function NotFound({ id }: { id: string }) {
  return (
    <div className="flex items-start gap-3 rounded-md border bg-muted/40 px-3 py-3 text-sm">
      <TriangleAlert
        size={14}
        className="mt-0.5 shrink-0 text-muted-foreground"
        aria-hidden
      />
      <div>
        <div className="font-medium text-foreground">Graph not found</div>
        <div className="mt-0.5 text-muted-foreground">
          No graph at <code className="font-mono">{id}</code>.
        </div>
      </div>
    </div>
  );
}

function Skeleton() {
  return (
    <div className="space-y-3">
      <div className="h-12 animate-pulse rounded-md bg-card" />
      <div className="grid gap-3 sm:grid-cols-3">
        {Array.from({ length: 3 }).map((_, i) => (
          <div key={i} className="h-16 animate-pulse rounded-md bg-card" />
        ))}
      </div>
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

function shortIri(iri: string | null | undefined): string {
  if (!iri) return "";
  const m = iri.match(/[#/]([^#/]+)$/);
  return m ? m[1] : iri;
}
