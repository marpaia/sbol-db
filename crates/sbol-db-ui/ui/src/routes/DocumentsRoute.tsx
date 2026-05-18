/**
 * Documents listing. Paginated table of every SBOL document the server
 * has imported, newest first. The "Import" button opens the dialog that
 * mirrors `POST /documents`; rows link into the per-document detail
 * page that surfaces validation status and the objects this document
 * produced.
 */

import { useCallback, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { ChevronLeft, ChevronRight, FileText, Plus } from "lucide-react";

import { DataTable, type DataTableColumn } from "@/components/lab/DataTable";
import { DocumentImportDialog } from "@/components/lab/DocumentImportDialog";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { useDocuments } from "@/hooks/useDocuments";
import type { DocumentSummary, ImportReport } from "@/lib/api";
import { formatRelative } from "@/lib/utils";

const PAGE_SIZE = 50;

export default function DocumentsRoute() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [page, setPage] = useState(0);
  const [importerOpen, setImporterOpen] = useState(false);

  const { data, isLoading, error } = useDocuments({
    limit: PAGE_SIZE,
    offset: page * PAGE_SIZE,
  });

  const totalPages = data ? Math.max(1, Math.ceil(data.total / PAGE_SIZE)) : 1;

  const onImported = useCallback(
    (report: ImportReport) => {
      queryClient.invalidateQueries({ queryKey: ["lab", "documents"] });
      queryClient.invalidateQueries({ queryKey: ["lab", "overview"] });
      // Jump straight to the detail page — closing the dialog from there
      // is the natural next step after a successful import.
      setImporterOpen(false);
      navigate(`/documents/${report.document_id}`);
    },
    [queryClient, navigate]
  );

  const columns: DataTableColumn<DocumentSummary>[] = [
    {
      id: "name",
      header: "Name",
      width: 280,
      cell: (d) => (
        <div className="flex items-center gap-2">
          <FileText
            size={12}
            className="shrink-0 text-muted-foreground/70"
            aria-hidden
          />
          <span className="truncate text-foreground">
            {d.name ?? <Muted>untitled</Muted>}
          </span>
        </div>
      ),
      sortValue: (d) => d.name?.toLowerCase() ?? "",
      filterValue: (d) => d.name ?? "",
    },
    {
      id: "format",
      header: "Format",
      width: 90,
      cell: (d) => (
        <span className="font-mono text-[11px] text-muted-foreground">
          {d.serialization_format}
        </span>
      ),
      sortValue: (d) => d.serialization_format,
      filterValue: (d) => d.serialization_format,
    },
    {
      id: "objects",
      header: "Objects",
      width: 90,
      align: "right",
      cell: (d) => d.object_count.toLocaleString(),
      sortValue: (d) => d.object_count,
    },
    {
      id: "source",
      header: "Source",
      width: 280,
      cell: (d) =>
        d.source_uri ? (
          <span className="truncate font-mono text-[11px] text-muted-foreground">
            {d.source_uri}
          </span>
        ) : (
          <Muted>—</Muted>
        ),
      sortValue: (d) => d.source_uri?.toLowerCase() ?? "",
      filterValue: (d) => d.source_uri ?? "",
    },
    {
      id: "created_at",
      header: "Imported",
      width: 110,
      align: "right",
      cell: (d) => (
        <span title={d.created_at}>{formatRelative(d.created_at)}</span>
      ),
      sortValue: (d) => d.created_at,
    },
  ];

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-6 px-8 py-10">
        <header className="flex flex-wrap items-start justify-between gap-4">
          <div>
            <h1 className="text-2xl font-semibold tracking-tight">Documents</h1>
            <p className="mt-2 text-sm text-muted-foreground">
              Every SBOL document the server has ingested. Each row commits
              objects, quads, and a validation run in one transaction.
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
            title="Couldn't list documents"
            body={(error as Error).message}
          />
        ) : isLoading && !data ? (
          <TableSkeleton />
        ) : !data || data.documents.length === 0 ? (
          <Empty onImport={() => setImporterOpen(true)} />
        ) : (
          <>
            <PageStatus total={data.total} page={page} pageSize={PAGE_SIZE} />
            <div className="overflow-hidden rounded-lg border bg-card">
              <DataTable
                columns={columns}
                rows={data.documents}
                rowKey={(d) => d.id}
                filterable
                onRowClick={(d) => navigate(`/documents/${d.id}`)}
              />
            </div>
            <Pagination page={page} totalPages={totalPages} onPage={setPage} />
          </>
        )}
      </div>

      <DocumentImportDialog
        open={importerOpen}
        onOpenChange={setImporterOpen}
        onImported={onImported}
      />
    </div>
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
      documents
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
      <FileText size={20} className="mx-auto text-muted-foreground/60" />
      <p className="mt-3 text-sm text-foreground">No documents yet.</p>
      <p className="mx-auto mt-1 max-w-md text-xs text-muted-foreground">
        Import a Turtle, JSON-LD, RDF/XML, or N-Triples file to get started.
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
