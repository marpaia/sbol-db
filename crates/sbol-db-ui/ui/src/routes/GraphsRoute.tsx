/**
 * Graphs listing. Paginated table of every named graph the server holds,
 * newest first. Shows both `sbol3` graphs (imported SBOL documents, with a
 * derived object view) and `verbatim` graphs (raw RDF written through the
 * SynBioHub-compatible Graph
 * Store / SPARQL Update endpoints). The "Import" button creates an `sbol3`
 * graph; rows link into the per-graph detail page.
 */

import { useCallback, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { ChevronLeft, ChevronRight, Plus, Share2 } from "lucide-react";

import { DataTable, type DataTableColumn } from "@/components/lab/DataTable";
import { ImportDialog } from "@/components/lab/ImportDialog";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { useGraphs } from "@/hooks/useGraphs";
import type { GraphSummary, ImportReport } from "@/lib/api";
import { formatRelative } from "@/lib/utils";

const PAGE_SIZE = 50;

export default function GraphsRoute() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [page, setPage] = useState(0);
  const [importerOpen, setImporterOpen] = useState(false);

  const { data, isLoading, error } = useGraphs({
    limit: PAGE_SIZE,
    offset: page * PAGE_SIZE,
  });

  const totalPages = data ? Math.max(1, Math.ceil(data.total / PAGE_SIZE)) : 1;

  const onImported = useCallback(
    (report: ImportReport) => {
      queryClient.invalidateQueries({ queryKey: ["lab", "graphs"] });
      queryClient.invalidateQueries({ queryKey: ["lab", "overview"] });
      setImporterOpen(false);
      navigate(`/graphs/${report.graph_id}`);
    },
    [queryClient, navigate]
  );

  const columns: DataTableColumn<GraphSummary>[] = [
    {
      id: "name",
      header: "Name / IRI",
      width: 320,
      cell: (g) => (
        <div className="min-w-0">
          {g.name ? (
            <div className="truncate text-foreground">{g.name}</div>
          ) : null}
          <div className="truncate font-mono text-[11px] text-muted-foreground">
            {g.iri}
          </div>
        </div>
      ),
      sortValue: (g) => g.name?.toLowerCase() ?? g.iri,
      filterValue: (g) => `${g.name ?? ""} ${g.iri}`,
    },
    {
      id: "kind",
      header: "Kind",
      width: 110,
      cell: (g) => <KindBadge kind={g.kind} />,
      sortValue: (g) => g.kind,
      filterValue: (g) => g.kind,
    },
    {
      id: "triples",
      header: "Triples",
      width: 90,
      align: "right",
      cell: (g) => g.triple_count.toLocaleString(),
      sortValue: (g) => g.triple_count,
    },
    {
      id: "objects",
      header: "Objects",
      width: 90,
      align: "right",
      cell: (g) =>
        g.object_count > 0 ? (
          g.object_count.toLocaleString()
        ) : (
          <Muted>—</Muted>
        ),
      sortValue: (g) => g.object_count,
    },
    {
      id: "created_at",
      header: "Created",
      width: 110,
      align: "right",
      cell: (g) => (
        <span title={g.created_at}>{formatRelative(g.created_at)}</span>
      ),
      sortValue: (g) => g.created_at,
    },
  ];

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-6 px-8 py-10">
        <header className="flex flex-wrap items-start justify-between gap-4">
          <div>
            <h1 className="text-2xl font-semibold tracking-tight">Graphs</h1>
            <p className="mt-2 text-sm text-muted-foreground">
              Every named graph in the store. <strong>SBOL3</strong> graphs are
              imported documents with a derived object view;{" "}
              <strong>verbatim</strong> graphs are raw RDF written through the
              triplestore endpoints.
            </p>
          </div>
          <button
            type="button"
            onClick={() => setImporterOpen(true)}
            className="inline-flex items-center gap-1.5 rounded-md bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90"
          >
            <Plus size={14} />
            Import
          </button>
        </header>

        {error ? (
          <ErrorBanner
            title="Couldn't list graphs"
            body={(error as Error).message}
          />
        ) : isLoading && !data ? (
          <TableSkeleton />
        ) : !data || data.graphs.length === 0 ? (
          <Empty onImport={() => setImporterOpen(true)} />
        ) : (
          <>
            <PageStatus total={data.total} page={page} pageSize={PAGE_SIZE} />
            <div className="overflow-hidden rounded-lg border bg-card">
              <DataTable
                columns={columns}
                rows={data.graphs}
                rowKey={(g) => g.id}
                filterable
                onRowClick={(g) => navigate(`/graphs/${g.id}`)}
              />
            </div>
            <Pagination page={page} totalPages={totalPages} onPage={setPage} />
          </>
        )}
      </div>

      <ImportDialog
        open={importerOpen}
        onOpenChange={setImporterOpen}
        onImported={onImported}
      />
    </div>
  );
}

function KindBadge({ kind }: { kind: GraphSummary["kind"] }) {
  const isSbol3 = kind === "sbol3";
  return (
    <span
      className={`inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wider ${
        isSbol3
          ? "bg-primary/10 text-primary"
          : "bg-muted text-muted-foreground"
      }`}
    >
      {isSbol3 ? "SBOL3" : "verbatim"}
    </span>
  );
}

function PageStatus({
  total,
  page,
  pageSize,
}: {
  total: number;
  page: number;
  pageSize: number;
}) {
  const start = total === 0 ? 0 : page * pageSize + 1;
  const end = Math.min((page + 1) * pageSize, total);
  return (
    <div className="text-xs text-muted-foreground">
      Showing{" "}
      <span className="tabular-nums text-foreground">
        {start.toLocaleString()}–{end.toLocaleString()}
      </span>{" "}
      of{" "}
      <span className="tabular-nums text-foreground">
        {total.toLocaleString()}
      </span>{" "}
      graphs
    </div>
  );
}

function Pagination({
  page,
  totalPages,
  onPage,
}: {
  page: number;
  totalPages: number;
  onPage: (p: number) => void;
}) {
  if (totalPages <= 1) return null;
  return (
    <div className="flex items-center justify-between gap-2">
      <button
        type="button"
        onClick={() => onPage(Math.max(0, page - 1))}
        disabled={page === 0}
        className="inline-flex items-center gap-1 rounded-md border px-2.5 py-1.5 text-xs font-medium transition-colors hover:bg-accent/40 disabled:cursor-not-allowed disabled:opacity-50"
      >
        <ChevronLeft size={12} />
        Previous
      </button>
      <div className="text-xs tabular-nums text-muted-foreground">
        Page {page + 1} of {totalPages}
      </div>
      <button
        type="button"
        onClick={() => onPage(Math.min(totalPages - 1, page + 1))}
        disabled={page >= totalPages - 1}
        className="inline-flex items-center gap-1 rounded-md border px-2.5 py-1.5 text-xs font-medium transition-colors hover:bg-accent/40 disabled:cursor-not-allowed disabled:opacity-50"
      >
        Next
        <ChevronRight size={12} />
      </button>
    </div>
  );
}

function Empty({ onImport }: { onImport: () => void }) {
  return (
    <div className="rounded-lg border bg-card px-6 py-10 text-center">
      <Share2 size={20} className="mx-auto text-muted-foreground/60" />
      <p className="mt-3 text-sm text-foreground">No graphs yet.</p>
      <p className="mx-auto mt-1 max-w-md text-xs text-muted-foreground">
        Import an SBOL document, or write RDF through the Graph Store endpoints,
        to populate the store.
      </p>
      <button
        type="button"
        onClick={onImport}
        className="mt-4 inline-flex items-center gap-1.5 rounded-md bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90"
      >
        <Plus size={14} />
        Import a document
      </button>
    </div>
  );
}

function TableSkeleton() {
  return (
    <div className="space-y-1">
      {Array.from({ length: 6 }).map((_, i) => (
        <div key={i} className="h-9 animate-pulse rounded-md bg-card" />
      ))}
    </div>
  );
}

function Muted({ children }: { children: React.ReactNode }) {
  return <span className="text-muted-foreground/60">{children}</span>;
}
