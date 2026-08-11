/**
 * Paginated table of every named RDF graph the server holds. Rows link into a
 * single graph detail representation backed by canonical triples.
 */

import { useCallback, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { Plus, Search, Share2 } from "lucide-react";

import { AdminPage } from "@/components/admin/AdminPage";
import { DataTable, type DataTableColumn } from "@/components/lab/DataTable";
import { ImportDialog } from "@/components/lab/ImportDialog";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { ProductEmptyState } from "@/components/product/ProductEmptyState";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { graphKeys, useGraphs } from "@/features/admin/graphs/queries";
import { overviewKeys } from "@/features/admin/overview/queries";
import type { GraphSummary } from "@/features/admin/graphs/api";
import type { ImportReport } from "@/features/admin/imports/api";
import { adminPath } from "@/lib/routes";
import { formatRelative } from "@/lib/utils";

const PAGE_SIZE = 50;

export default function GraphsRoute() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [cursors, setCursors] = useState<string[]>([""]);
  const after = cursors.at(-1) || undefined;
  const page = cursors.length - 1;
  const [importerOpen, setImporterOpen] = useState(false);
  const [searchDraft, setSearchDraft] = useState("");
  const [search, setSearch] = useState("");

  const { data, isLoading, error } = useGraphs({
    limit: PAGE_SIZE,
    after,
    q: search || undefined,
  });

  const onImported = useCallback(
    (report: ImportReport) => {
      queryClient.invalidateQueries({ queryKey: graphKeys.all });
      queryClient.invalidateQueries({ queryKey: overviewKeys.all });
      setImporterOpen(false);
      navigate(adminPath(`/graphs/${report.graph_id}`));
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
      id: "triples",
      header: "Triples",
      width: 90,
      align: "right",
      cell: (g) =>
        g.triple_count === null ? (
          <Muted>—</Muted>
        ) : (
          g.triple_count.toLocaleString()
        ),
      sortValue: (g) => g.triple_count ?? -1,
    },
    {
      id: "created_at",
      header: "Created",
      width: 110,
      align: "right",
      cell: (g) =>
        g.created_at ? (
          <span title={g.created_at}>{formatRelative(g.created_at)}</span>
        ) : (
          <Muted>Not catalogued</Muted>
        ),
      sortValue: (g) => g.created_at ?? "",
    },
  ];

  return (
    <>
      <AdminPage
        title="Graphs"
        description="Every named RDF graph in the canonical store, regardless of how it was created."
        eyebrow="Data model"
        action={
          <Button size="sm" type="button" onClick={() => setImporterOpen(true)}>
            <Plus />
            Import
          </Button>
        }
      >
        <form
          className="flex gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            setSearch(searchDraft.trim());
            setCursors([""]);
          }}
        >
          <div className="relative min-w-0 flex-1">
            <Search className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={searchDraft}
              onChange={(event) => setSearchDraft(event.target.value)}
              className="pl-9"
              placeholder="Search graph name or IRI"
              aria-label="Search graphs"
            />
          </div>
          <Button type="submit" variant="outline">
            Search
          </Button>
          {(search || searchDraft) && (
            <Button
              type="button"
              variant="ghost"
              onClick={() => {
                setSearchDraft("");
                setSearch("");
                setCursors([""]);
              }}
            >
              Clear
            </Button>
          )}
        </form>
        {error ? (
          <ErrorBanner
            title="Couldn't list graphs"
            body={(error as Error).message}
          />
        ) : isLoading && !data ? (
          <TableSkeleton />
        ) : !data || data.items.length === 0 ? (
          <Empty searching={!!search} onImport={() => setImporterOpen(true)} />
        ) : (
          <>
            <div className="text-xs text-muted-foreground">
              Page{" "}
              <span className="tabular-nums text-foreground">{page + 1}</span>
              {" · "}
              <span className="tabular-nums text-foreground">
                {data.items.length.toLocaleString()}
              </span>{" "}
              graphs
              {!data.next_cursor && " · end of corpus"}
            </div>
            <div className="overflow-hidden rounded-lg border bg-card">
              <DataTable
                columns={columns}
                rows={data.items}
                rowKey={(g) => g.id}
                onRowClick={(g) => navigate(adminPath(`/graphs/${g.id}`))}
              />
            </div>
            <div className="flex items-center justify-end gap-2 text-xs">
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
          </>
        )}
      </AdminPage>

      <ImportDialog
        open={importerOpen}
        onOpenChange={setImporterOpen}
        onImported={onImported}
      />
    </>
  );
}

function Empty({
  searching,
  onImport,
}: {
  searching: boolean;
  onImport: () => void;
}) {
  return (
    <ProductEmptyState
      density="compact"
      icon={Share2}
      title={searching ? "No matching graphs" : "No graphs yet"}
      description={
        searching
          ? "Try a different graph name or IRI."
          : "Import an SBOL document, or write RDF through the Graph Store endpoints, to populate the store."
      }
      action={
        !searching ? (
          <Button type="button" size="sm" onClick={onImport}>
            <Plus />
            Import a document
          </Button>
        ) : undefined
      }
    />
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
