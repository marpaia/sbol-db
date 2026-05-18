/**
 * Per-document detail. Shows the import record (name, source URI,
 * serialization format, who imported it, when) alongside KPI tiles for
 * the object and quad counts, then a paginated listing of every object
 * that this document produced. Object rows link into the typed object
 * detail page.
 */

import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChevronLeft, FileText, TriangleAlert } from "lucide-react";
import { Link, useNavigate, useParams } from "react-router-dom";

import { DataTable, type DataTableColumn } from "@/components/lab/DataTable";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { KpiTile } from "@/components/observability/KpiTile";
import { useDocument } from "@/hooks/useDocuments";
import { ApiError, listObjects, type SbolObjectRecord } from "@/lib/api";
import { formatRelative } from "@/lib/utils";

const PAGE_SIZE = 100;

export default function DocumentDetailRoute() {
  const navigate = useNavigate();
  const params = useParams<{ id: string }>();
  const id = params.id ?? "";

  const { data, isLoading, error } = useDocument(id);

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-6 px-8 py-10">
        <Link
          to="/documents"
          className="inline-flex items-center gap-1 text-xs text-muted-foreground transition-colors hover:text-foreground"
        >
          <ChevronLeft size={12} />
          All documents
        </Link>

        {error instanceof ApiError && error.status === 404 ? (
          <NotFound id={id} />
        ) : error ? (
          <ErrorBanner
            title="Couldn't load document"
            body={(error as Error).message}
          />
        ) : isLoading || !data ? (
          <Skeleton />
        ) : (
          <>
            <Header detail={data} />
            <div className="grid gap-3 sm:grid-cols-3">
              <KpiTile
                label="Objects"
                value={data.object_count.toLocaleString()}
              />
              <KpiTile label="Quads" value={data.quad_count.toLocaleString()} />
              <KpiTile label="Format" value={data.serialization_format} />
            </div>
            <Metadata detail={data} />
            <section>
              <SectionLabel>Objects in this document</SectionLabel>
              <ObjectsForDocument
                documentId={data.id}
                onOpen={(iri) =>
                  navigate(`/objects/${encodeURIComponent(iri)}`)
                }
              />
            </section>
          </>
        )}
      </div>
    </div>
  );
}

function Header({
  detail,
}: {
  detail: {
    name: string | null;
    document_iri: string | null;
    description: string | null;
  };
}) {
  return (
    <header className="space-y-1.5">
      <div className="flex items-center gap-2">
        <FileText size={16} className="text-muted-foreground/70" aria-hidden />
        <h1 className="text-2xl font-semibold tracking-tight">
          {detail.name ?? "Untitled document"}
        </h1>
      </div>
      {detail.document_iri && (
        <div className="truncate font-mono text-[11px] text-muted-foreground/80">
          {detail.document_iri}
        </div>
      )}
      {detail.description && (
        <p className="text-sm text-muted-foreground">{detail.description}</p>
      )}
    </header>
  );
}

function Metadata({
  detail,
}: {
  detail: {
    source_uri: string | null;
    created_by: string | null;
    created_at: string;
  };
}) {
  return (
    <section className="rounded-lg border bg-card px-4 py-3">
      <dl className="grid gap-3 text-sm sm:grid-cols-3">
        <Pair label="Source URI" value={detail.source_uri} mono />
        <Pair label="Created by" value={detail.created_by} />
        <Pair
          label="Imported"
          value={`${formatRelative(detail.created_at)} (${new Date(detail.created_at).toLocaleString()})`}
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

function ObjectsForDocument({
  documentId,
  onOpen,
}: {
  documentId: string;
  onOpen: (iri: string) => void;
}) {
  const [cursors, setCursors] = useState<string[]>([""]);
  const after = cursors[cursors.length - 1] || undefined;
  const page = cursors.length - 1;

  const { data, isLoading, error } = useQuery({
    queryKey: ["lab", "objects", "by-doc", documentId, after ?? ""],
    queryFn: ({ signal }) =>
      listObjects({ document_id: documentId, limit: PAGE_SIZE, after }, signal),
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
      filterValue: (o) =>
        `${o.display_id ?? ""} ${o.iri} ${o.name ?? ""}`.trim(),
    },
    {
      id: "name",
      header: "Name",
      width: 200,
      cell: (o) =>
        o.name ?? <span className="text-muted-foreground/60">—</span>,
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
        No objects projected from this document yet.
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

function NotFound({ id }: { id: string }) {
  return (
    <div className="flex items-start gap-3 rounded-md border bg-muted/40 px-3 py-3 text-sm">
      <TriangleAlert
        size={14}
        className="mt-0.5 shrink-0 text-muted-foreground"
        aria-hidden
      />
      <div>
        <div className="font-medium text-foreground">Document not found</div>
        <div className="mt-0.5 text-muted-foreground">
          No document at <code className="font-mono">{id}</code>.
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
