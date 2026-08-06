/**
 * RDF resource browser. The universal catalog applies text/class/role filters
 * before a bounded keyset page, so the route stays cheap regardless of corpus
 * size or storage backend.
 */

import { useEffect, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { Boxes, ChevronDown, ChevronUp, Filter, Search } from "lucide-react";

import { AdminPage } from "@/components/admin/AdminPage";
import { DataTable, type DataTableColumn } from "@/components/lab/DataTable";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { ProductEmptyState } from "@/components/product/ProductEmptyState";
import { useObjectsList } from "@/features/admin/objects/queries";
import type { SbolObjectRecord } from "@/features/admin/objects/api";
import { adminPath } from "@/lib/routes";

const PAGE_SIZE = 100;

export default function ObjectsRoute() {
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();

  const classFilter = searchParams.get("class") ?? "";
  const roleFilter = searchParams.get("role") ?? "";
  const textFilter = searchParams.get("q") ?? "";

  const [cursors, setCursors] = useState<string[]>([""]);
  const after = cursors[cursors.length - 1] || undefined;
  const page = cursors.length - 1;

  const { data, isLoading, error } = useObjectsList({
    class: classFilter || undefined,
    role: roleFilter || undefined,
    q: textFilter || undefined,
    after,
    limit: PAGE_SIZE,
  });

  const updateFilters = (filters: {
    q: string;
    class: string;
    role: string;
  }) => {
    const next = new URLSearchParams(searchParams);
    for (const [key, value] of Object.entries(filters)) {
      if (value.trim()) next.set(key, value.trim());
      else next.delete(key);
    }
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
          {firstValue(o.meta.display_id) && (
            <div className="truncate text-foreground">
              {firstValue(o.meta.display_id)}
            </div>
          )}
          <div className="truncate font-mono text-[11px] text-muted-foreground">
            {o.iri}
          </div>
        </div>
      ),
      sortValue: (o) => o.iri,
      filterValue: (o) =>
        `${firstValue(o.meta.display_id)} ${o.iri} ${firstValue(o.meta.name)}`.trim(),
    },
    {
      id: "name",
      header: "Name",
      width: 180,
      cell: (o) =>
        firstValue(o.meta.name) || (
          <span className="text-muted-foreground/60">—</span>
        ),
      sortValue: (o) => firstValue(o.meta.name).toLowerCase(),
    },
    {
      id: "class",
      header: "Class",
      width: 200,
      cell: (o) => (
        <span className="font-mono text-[11px] text-muted-foreground">
          {shortIri(o.meta.types?.[0])}
        </span>
      ),
      sortValue: (o) => o.meta.types?.[0] ?? "",
      filterValue: (o) => o.meta.types?.join(" ") ?? "",
    },
    {
      id: "version",
      header: "Version",
      width: 90,
      cell: (o) =>
        firstValue(o.meta.version) || (
          <span className="text-muted-foreground/60">—</span>
        ),
      sortValue: (o) => firstValue(o.meta.version),
    },
    {
      id: "graphs",
      header: "Graphs",
      width: 80,
      align: "right",
      cell: (o) => o.graph_count.toLocaleString(),
      sortValue: (o) => o.graph_count,
    },
  ];

  return (
    <AdminPage
      title="Resources"
      description="RDF subject resources in the canonical corpus. Search by identity or metadata, filter by class or role, and inspect graph-scoped occurrences."
      eyebrow="Data model"
    >
      <Filters
        classFilter={classFilter}
        roleFilter={roleFilter}
        textFilter={textFilter}
        onApply={updateFilters}
      />

      {error ? (
        <ErrorBanner
          title="Couldn't list resources"
          body={(error as Error).message}
        />
      ) : isLoading && !data ? (
        <TableSkeleton />
      ) : !data || data.items.length === 0 ? (
        <Empty hasFilters={!!(textFilter || classFilter || roleFilter)} />
      ) : (
        <>
          <div className="text-xs text-muted-foreground">
            Page{" "}
            <span className="tabular-nums text-foreground">{page + 1}</span>
            {" · "}
            <span className="tabular-nums text-foreground">
              {data.items.length.toLocaleString()}
            </span>{" "}
            resources
            {!data.next_cursor && " · end of corpus"}
          </div>
          <div className="overflow-hidden rounded-lg border bg-card">
            <DataTable
              columns={columns}
              rows={data.items}
              rowKey={(o) => o.iri}
              onRowClick={(o) =>
                navigate(adminPath(`/objects/${encodeURIComponent(o.iri)}`))
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
    </AdminPage>
  );
}

function Filters({
  classFilter,
  roleFilter,
  textFilter,
  onApply,
}: {
  classFilter: string;
  roleFilter: string;
  textFilter: string;
  onApply: (filters: { q: string; class: string; role: string }) => void;
}) {
  const [open, setOpen] = useState(!!(textFilter || classFilter || roleFilter));
  const [draft, setDraft] = useState({
    q: textFilter,
    class: classFilter,
    role: roleFilter,
  });
  const active = [textFilter, classFilter, roleFilter].filter(Boolean).length;

  useEffect(() => {
    setDraft({ q: textFilter, class: classFilter, role: roleFilter });
  }, [classFilter, roleFilter, textFilter]);

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
        <form
          className="grid gap-3 border-t px-4 py-3 sm:grid-cols-2"
          onSubmit={(event) => {
            event.preventDefault();
            onApply(draft);
          }}
        >
          <FilterField
            label="Text or resource IRI"
            value={draft.q}
            placeholder="sequence, promoter, or https://…"
            onChange={(q) => setDraft((value) => ({ ...value, q }))}
            icon="search"
          />
          <FilterField
            label="RDF class IRI"
            value={draft.class}
            placeholder="http://sbols.org/v2#ComponentDefinition"
            onChange={(value) =>
              setDraft((filters) => ({ ...filters, class: value }))
            }
          />
          <FilterField
            label="Role IRI"
            value={draft.role}
            placeholder="http://identifiers.org/so/SO:0000167"
            onChange={(role) => setDraft((value) => ({ ...value, role }))}
          />
          <div className="flex items-end justify-end gap-2">
            <button
              type="button"
              onClick={() => {
                const empty = { q: "", class: "", role: "" };
                setDraft(empty);
                onApply(empty);
              }}
              disabled={active === 0 && !Object.values(draft).some(Boolean)}
              className="rounded-md px-2.5 py-1.5 text-xs font-medium text-muted-foreground transition-colors hover:bg-accent disabled:opacity-50"
            >
              Clear
            </button>
            <button
              type="submit"
              className="rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90"
            >
              Apply filters
            </button>
          </div>
        </form>
      )}
    </section>
  );
}

function FilterField({
  label,
  value,
  placeholder,
  onChange,
  icon,
}: {
  label: string;
  value: string;
  placeholder: string;
  onChange: (v: string) => void;
  icon?: "search";
}) {
  return (
    <label className="block">
      <span className="mb-1 block text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
        {label}
      </span>
      <span className="relative block">
        {icon === "search" && (
          <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        )}
        <input
          type="text"
          value={value}
          placeholder={placeholder}
          onChange={(e) => onChange(e.target.value)}
          className={`w-full rounded-md border bg-background py-1.5 pr-3 font-mono text-[11px] text-foreground outline-none placeholder:text-muted-foreground/60 focus:ring-1 focus:ring-ring ${icon ? "pl-8" : "pl-3"}`}
        />
      </span>
    </label>
  );
}

function Empty({ hasFilters }: { hasFilters: boolean }) {
  return (
    <ProductEmptyState
      density="compact"
      icon={Boxes}
      title={
        hasFilters
          ? "No resources match the current filters"
          : "No typed resources in the database yet"
      }
      description={
        hasFilters
          ? "Try clearing one of the filter fields."
          : "Import RDF to populate the corpus."
      }
    />
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

function firstValue(values?: Array<{ value: string }>): string {
  return values?.[0]?.value ?? "";
}
