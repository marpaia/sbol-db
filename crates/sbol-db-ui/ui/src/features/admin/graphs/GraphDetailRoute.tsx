/**
 * Per-graph detail. Every graph uses one representation: provenance followed
 * by a paginated view of its canonical RDF triples.
 */

import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChevronLeft, Share2, TriangleAlert } from "lucide-react";
import { Link, useParams } from "react-router-dom";

import { DataTable, type DataTableColumn } from "@/components/lab/DataTable";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { KpiTile } from "@/components/observability/KpiTile";
import { graphKeys, useGraph } from "@/features/admin/graphs/queries";
import {
  listGraphTriples,
  type GraphTriple,
  type GraphSummary,
  type GraphTerm,
} from "@/features/admin/graphs/api";
import { HttpError } from "@/shared/api/http";
import { formatRelative } from "@/lib/utils";
import { adminPath } from "@/lib/routes";

export default function GraphDetailRoute() {
  const params = useParams<{ id: string }>();
  const id = params.id ?? "";

  const { data, isLoading, error } = useGraph(id);

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-6 px-8 py-10">
        <Link
          to={adminPath("/graphs")}
          className="inline-flex items-center gap-1 text-xs text-muted-foreground transition-colors hover:text-foreground"
        >
          <ChevronLeft size={12} />
          All graphs
        </Link>

        {error instanceof HttpError && error.status === 404 ? (
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
            <div className="grid gap-3 sm:grid-cols-2">
              <KpiTile
                label="Triples"
                value={data.triple_count?.toLocaleString() ?? "Not catalogued"}
              />
              <KpiTile
                label="Format"
                value={data.serialization_format ?? "—"}
              />
            </div>
            <Metadata graph={data} />
            <section>
              <SectionLabel>Triples in this graph</SectionLabel>
              <TriplesForGraph graphId={data.id} />
            </section>
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
      <dl className="grid gap-3 text-sm sm:grid-cols-2">
        <Pair label="Source URI" value={graph.source_uri} mono />
        <Pair
          label="Created"
          value={
            graph.created_at
              ? `${formatRelative(graph.created_at)} (${new Date(graph.created_at).toLocaleString()})`
              : null
          }
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

const TRIPLES_PAGE_SIZE = 100;

function TriplesForGraph({ graphId }: { graphId: string }) {
  const [cursors, setCursors] = useState<string[]>([""]);
  const after = cursors.at(-1) || undefined;
  const page = cursors.length - 1;

  const { data, isLoading, error } = useQuery({
    queryKey: graphKeys.triples(graphId, after),
    queryFn: ({ signal }) =>
      listGraphTriples(graphId, { limit: TRIPLES_PAGE_SIZE, after }, signal),
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
  if (!data || data.items.length === 0) {
    return (
      <div className="rounded-lg border bg-card px-4 py-6 text-sm text-muted-foreground">
        This graph has no triples.
      </div>
    );
  }

  return (
    <div className="space-y-3">
      <div className="overflow-hidden rounded-lg border bg-card">
        <DataTable
          columns={columns}
          rows={data.items}
          rowKey={(q) =>
            `${q.subject.value} ${q.predicate.value} ${q.object.value}`
          }
          filterable
        />
      </div>
      <div className="flex items-center justify-between gap-2 text-xs">
        <div className="text-muted-foreground">
          Page {page + 1} · {data.items.length.toLocaleString()} triples
          {!data.next_cursor && " · end of graph"}
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => setCursors((values) => values.slice(0, -1))}
            disabled={page === 0}
            className="rounded-md border px-2.5 py-1 font-medium transition-colors hover:bg-accent/40 disabled:cursor-not-allowed disabled:opacity-50"
          >
            Previous
          </button>
          <button
            type="button"
            onClick={() =>
              data.next_cursor &&
              setCursors((values) => [...values, data.next_cursor!])
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
