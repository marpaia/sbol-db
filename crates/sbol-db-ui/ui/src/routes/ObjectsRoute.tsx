/**
 * Typed object browser. Paginated listing over `GET /objects/list`,
 * with optional `sbol_class`, `role`, and `document_id` filters surfaced
 * as collapsible form inputs. The list uses the server's keyset cursor
 * so the table stays cheap regardless of corpus size.
 */

import { useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { ChevronDown, ChevronUp, Filter, Search } from "lucide-react";

import { DataTable, type DataTableColumn } from "@/components/lab/DataTable";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { useObjectsList } from "@/hooks/useObjects";
import type { SbolObjectRecord } from "@/lib/api";

const PAGE_SIZE = 100;

export default function ObjectsRoute() {
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();

  const classFilter = searchParams.get("class") ?? "";
  const roleFilter = searchParams.get("role") ?? "";
  const docFilter = searchParams.get("document") ?? "";

  const [cursors, setCursors] = useState<string[]>([""]);
  const after = cursors[cursors.length - 1] || undefined;
  const page = cursors.length - 1;

  const { data, isLoading, error } = useObjectsList({
    sbol_class: classFilter || undefined,
    role: roleFilter || undefined,
    document_id: docFilter || undefined,
    after,
    limit: PAGE_SIZE,
  });

  const updateFilter = (key: "class" | "role" | "document", value: string) => {
    const next = new URLSearchParams(searchParams);
    if (value) next.set(key, value);
    else next.delete(key);
    setSearchParams(next, { replace: true });
    setCursors([""]);
  };

  const columns: DataTableColumn<SbolObjectRecord>[] = [
    {
      id: "display",
      header: "Display ID / IRI",
      width: 360,
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
      width: 180,
      cell: (o) =>
        o.name ?? <span className="text-muted-foreground/60">—</span>,
      sortValue: (o) => o.name?.toLowerCase() ?? "",
    },
    {
      id: "class",
      header: "Class",
      width: 200,
      cell: (o) => (
        <span className="font-mono text-[11px] text-muted-foreground">
          {shortIri(o.sbol_class)}
        </span>
      ),
      sortValue: (o) => o.sbol_class ?? "",
      filterValue: (o) => o.sbol_class ?? "",
    },
    {
      id: "version",
      header: "Version",
      width: 90,
      cell: (o) =>
        o.version ?? <span className="text-muted-foreground/60">—</span>,
      sortValue: (o) => o.version ?? "",
    },
  ];

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-6 px-8 py-10">
        <header className="flex flex-wrap items-start justify-between gap-4">
          <div>
            <h1 className="text-2xl font-semibold tracking-tight">Objects</h1>
            <p className="mt-2 text-sm text-muted-foreground">
              Every typed SBOL object in the database. Filter by class or role,
              or use the bulk lookup tool to resolve many IRIs at once.
            </p>
          </div>
          <button
            type="button"
            onClick={() => navigate("/objects/lookup")}
            className="inline-flex items-center gap-1.5 rounded-md border bg-background px-3 py-1.5 text-sm font-medium transition-colors hover:bg-accent/40"
          >
            <Search size={14} />
            Bulk lookup
          </button>
        </header>

        <Filters
          classFilter={classFilter}
          roleFilter={roleFilter}
          docFilter={docFilter}
          onChange={updateFilter}
        />

        {error ? (
          <ErrorBanner
            title="Couldn't list objects"
            body={(error as Error).message}
          />
        ) : isLoading && !data ? (
          <TableSkeleton />
        ) : !data || data.objects.length === 0 ? (
          <Empty hasFilters={!!(classFilter || roleFilter || docFilter)} />
        ) : (
          <>
            <div className="text-xs text-muted-foreground">
              Page{" "}
              <span className="tabular-nums text-foreground">{page + 1}</span>
              {" · "}
              <span className="tabular-nums text-foreground">
                {data.objects.length.toLocaleString()}
              </span>{" "}
              objects
              {!data.next_cursor && " · end of corpus"}
            </div>
            <div className="overflow-hidden rounded-lg border bg-card">
              <DataTable
                columns={columns}
                rows={data.objects}
                rowKey={(o) => o.id}
                filterable
                onRowClick={(o) =>
                  navigate(`/objects/${encodeURIComponent(o.iri)}`)
                }
              />
            </div>
            <div className="flex items-center justify-between gap-2 text-xs">
              <div className="text-muted-foreground">
                Keyset paginated. Sort is server-side (lexicographic IRI).
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
          </>
        )}
      </div>
    </div>
  );
}

function Filters({
  classFilter,
  roleFilter,
  docFilter,
  onChange,
}: {
  classFilter: string;
  roleFilter: string;
  docFilter: string;
  onChange: (key: "class" | "role" | "document", value: string) => void;
}) {
  const [open, setOpen] = useState(!!(classFilter || roleFilter || docFilter));
  const active = [classFilter, roleFilter, docFilter].filter(Boolean).length;
  return (
    <section className="rounded-lg border bg-card">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-2 px-4 py-2.5 text-left text-sm transition-colors hover:bg-accent/40"
      >
        <Filter size={14} className="text-muted-foreground/70" />
        <span className="font-medium text-foreground">Filters</span>
        {active > 0 && (
          <span className="rounded-sm bg-primary/15 px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wider text-primary">
            {active} active
          </span>
        )}
        {open ? (
          <ChevronUp size={14} className="ml-auto text-muted-foreground" />
        ) : (
          <ChevronDown size={14} className="ml-auto text-muted-foreground" />
        )}
      </button>
      {open && (
        <div className="grid gap-3 border-t px-4 py-3 sm:grid-cols-3">
          <FilterField
            label="sbol:class IRI"
            value={classFilter}
            placeholder="http://sbols.org/v3#Component"
            onChange={(v) => onChange("class", v)}
          />
          <FilterField
            label="Role IRI"
            value={roleFilter}
            placeholder="http://identifiers.org/so/SO:0000167"
            onChange={(v) => onChange("role", v)}
          />
          <FilterField
            label="Document ID"
            value={docFilter}
            placeholder="UUID"
            onChange={(v) => onChange("document", v)}
          />
        </div>
      )}
    </section>
  );
}

function FilterField({
  label,
  value,
  placeholder,
  onChange,
}: {
  label: string;
  value: string;
  placeholder: string;
  onChange: (v: string) => void;
}) {
  return (
    <label className="block">
      <span className="mb-1 block text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
        {label}
      </span>
      <input
        type="text"
        value={value}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
        className="w-full rounded-md border bg-background px-3 py-1.5 font-mono text-[11px] text-foreground outline-none placeholder:text-muted-foreground/60 focus:ring-1 focus:ring-ring"
      />
    </label>
  );
}

function Empty({ hasFilters }: { hasFilters: boolean }) {
  return (
    <div className="rounded-lg border bg-card px-6 py-10 text-center">
      <p className="text-sm text-foreground">
        {hasFilters
          ? "No objects match the current filters."
          : "No objects in the database yet."}
      </p>
      <p className="mx-auto mt-1 max-w-md text-xs text-muted-foreground">
        {hasFilters
          ? "Try clearing one of the filter fields."
          : "Import a document to populate the corpus."}
      </p>
    </div>
  );
}

function TableSkeleton() {
  return (
    <div className="space-y-1">
      {Array.from({ length: 8 }).map((_, i) => (
        <div key={i} className="h-10 animate-pulse rounded-md bg-card" />
      ))}
    </div>
  );
}

function shortIri(iri: string | null | undefined): string {
  if (!iri) return "";
  const m = iri.match(/[#/]([^#/]+)$/);
  return m ? m[1] : iri;
}
