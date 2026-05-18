/**
 * Bulk object lookup. Paste up to 1000 IRIs (one per line); the server
 * resolves them in a single `WHERE iri = ANY(...)` round trip and the
 * response separates resolved records from missing IRIs. Each side
 * renders as its own `DataTable`.
 */

import { useMemo, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { ChevronLeft, Loader2, Search } from "lucide-react";

import { DataTable, type DataTableColumn } from "@/components/lab/DataTable";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { useObjectLookup } from "@/hooks/useObjects";
import type { SbolObjectRecord } from "@/lib/api";
import { describeError } from "@/lib/utils";

const HARD_LIMIT = 1000;

export default function ObjectLookupRoute() {
  const navigate = useNavigate();
  const [text, setText] = useState("");
  const lookup = useObjectLookup();

  const iris = useMemo(
    () =>
      text
        .split(/\r?\n/)
        .map((s) => s.trim())
        .filter((s) => s.length > 0),
    [text]
  );
  const tooMany = iris.length > HARD_LIMIT;

  const onSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (tooMany || iris.length === 0) return;
    lookup.mutate(iris);
  };

  const foundColumns: DataTableColumn<SbolObjectRecord>[] = [
    {
      id: "iri",
      header: "IRI",
      width: 460,
      cell: (o) => (
        <span className="truncate font-mono text-[11px] text-foreground">
          {o.iri}
        </span>
      ),
      sortValue: (o) => o.iri,
      filterValue: (o) => o.iri,
    },
    {
      id: "display",
      header: "Display ID",
      width: 180,
      cell: (o) =>
        o.display_id ?? <span className="text-muted-foreground/60">—</span>,
      sortValue: (o) => o.display_id ?? "",
      filterValue: (o) => o.display_id ?? "",
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

  const missingColumns: DataTableColumn<{ iri: string }>[] = [
    {
      id: "iri",
      header: "IRI",
      width: 800,
      cell: (m) => (
        <span className="truncate font-mono text-[11px] text-foreground">
          {m.iri}
        </span>
      ),
      sortValue: (m) => m.iri,
      filterValue: (m) => m.iri,
    },
  ];

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-6 px-8 py-10">
        <Link
          to="/objects"
          className="inline-flex items-center gap-1 text-xs text-muted-foreground transition-colors hover:text-foreground"
        >
          <ChevronLeft size={12} />
          Object browser
        </Link>

        <header>
          <h1 className="text-2xl font-semibold tracking-tight">
            Bulk object lookup
          </h1>
          <p className="mt-2 text-sm text-muted-foreground">
            Paste up to {HARD_LIMIT.toLocaleString()} SBOL IRIs (one per line).
            The server resolves them in one round trip and returns matches
            alongside any IRIs that didn't hit.
          </p>
        </header>

        <form onSubmit={onSubmit} className="space-y-3">
          <textarea
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder={
              "http://example.com/foo\nhttp://example.com/bar\nhttp://example.com/baz"
            }
            rows={10}
            spellCheck={false}
            disabled={lookup.isPending}
            className="block w-full resize-y rounded-md border bg-background px-3 py-2 font-mono text-xs text-foreground outline-none placeholder:text-muted-foreground/60 focus:ring-1 focus:ring-ring disabled:opacity-50"
          />
          <div className="flex items-center justify-between gap-2 text-xs">
            <div className="text-muted-foreground">
              <span
                className={`tabular-nums ${
                  tooMany ? "text-destructive" : "text-foreground"
                }`}
              >
                {iris.length.toLocaleString()}
              </span>{" "}
              of {HARD_LIMIT.toLocaleString()} IRIs
              {tooMany && (
                <span className="ml-2 text-destructive">
                  Trim the list to submit.
                </span>
              )}
            </div>
            <button
              type="submit"
              disabled={tooMany || iris.length === 0 || lookup.isPending}
              className="inline-flex items-center gap-1.5 rounded-md bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:bg-muted disabled:text-muted-foreground"
            >
              {lookup.isPending ? (
                <Loader2 size={14} className="animate-spin" />
              ) : (
                <Search size={14} />
              )}
              {lookup.isPending ? "Resolving" : "Resolve"}
            </button>
          </div>
        </form>

        {lookup.error && (
          <ErrorBanner
            title="Lookup failed"
            body={describeError(lookup.error)}
          />
        )}

        {lookup.data && (
          <div className="space-y-6">
            <section>
              <SectionLabel>
                Found
                <span className="ml-2 tabular-nums text-muted-foreground">
                  {lookup.data.found.length.toLocaleString()}
                </span>
              </SectionLabel>
              {lookup.data.found.length === 0 ? (
                <div className="rounded-lg border bg-card px-4 py-6 text-sm text-muted-foreground">
                  No matches.
                </div>
              ) : (
                <div className="overflow-hidden rounded-lg border bg-card">
                  <DataTable
                    columns={foundColumns}
                    rows={lookup.data.found}
                    rowKey={(o) => o.id}
                    filterable
                    onRowClick={(o) =>
                      navigate(`/objects/${encodeURIComponent(o.iri)}`)
                    }
                  />
                </div>
              )}
            </section>

            <section>
              <SectionLabel>
                Missing
                <span className="ml-2 tabular-nums text-muted-foreground">
                  {lookup.data.missing.length.toLocaleString()}
                </span>
              </SectionLabel>
              {lookup.data.missing.length === 0 ? (
                <div className="rounded-lg border bg-card px-4 py-6 text-sm text-muted-foreground">
                  Every IRI resolved.
                </div>
              ) : (
                <div className="overflow-hidden rounded-lg border bg-card">
                  <DataTable
                    columns={missingColumns}
                    rows={lookup.data.missing.map((iri) => ({ iri }))}
                    rowKey={(m) => m.iri}
                    filterable
                  />
                </div>
              )}
            </section>
          </div>
        )}
      </div>
    </div>
  );
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
