import { ChevronLeft, ChevronRight, SlidersHorizontal } from "lucide-react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";

import { ObjectResultCard } from "@/components/portal/ObjectResultCard";
import { SearchBox } from "@/components/portal/SearchBox";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { usePortalSearch } from "@/features/portal/queries";

const PAGE_SIZE = 24;

export default function SearchRoute() {
  const navigate = useNavigate();
  const legacyQuery = useParams()["*"]?.trim() || "";
  const [params, setParams] = useSearchParams();
  const query = params.get("q")?.trim() || legacyQuery;
  const type = params.get("type")?.trim() || "";
  const offset = positiveInteger(params.get("offset"));
  const results = usePortalSearch({ q: query, type, offset, limit: PAGE_SIZE });

  const setQuery = (nextQuery: string) => {
    const next = new URLSearchParams(params);
    if (nextQuery) next.set("q", nextQuery);
    else next.delete("q");
    next.delete("offset");
    navigate({ pathname: "/search", search: next.toString() });
  };

  const setType = (nextType: string) => {
    const next = new URLSearchParams(params);
    if (nextType.trim()) next.set("type", nextType.trim());
    else next.delete("type");
    next.delete("offset");
    setParams(next, { replace: true });
  };

  const move = (nextOffset: number) => {
    const next = new URLSearchParams(params);
    if (nextOffset > 0) next.set("offset", String(nextOffset));
    else next.delete("offset");
    setParams(next);
    window.scrollTo({ top: 0 });
  };

  return (
    <div className="mx-auto w-full max-w-7xl px-4 py-10 sm:px-6 lg:px-8">
      <div className="max-w-3xl">
        <p className="text-xs font-medium uppercase tracking-[0.16em] text-primary">
          Discover
        </p>
        <h1 className="mt-2 text-3xl font-semibold tracking-tight">
          {query ? `Results for “${query}”` : "Browse the registry"}
        </h1>
        <p className="mt-2 text-sm leading-6 text-muted-foreground">
          Search the objects visible to your current account. Open any result to
          inspect its SBOL identity and download a machine-readable form.
        </p>
      </div>

      <SearchBox
        initialQuery={query}
        onSearch={setQuery}
        className="mt-7 max-w-3xl"
        autoFocus={!query}
      />

      <div className="mt-4 flex max-w-3xl items-center gap-3 rounded-lg border bg-muted/20 p-3">
        <SlidersHorizontal className="size-4 shrink-0 text-muted-foreground" />
        <label htmlFor="object-type" className="shrink-0 text-xs font-medium">
          SBOL type
        </label>
        <Input
          id="object-type"
          value={type}
          onChange={(event) => setType(event.target.value)}
          placeholder="Full rdf:type IRI (optional)"
          className="h-8 bg-background font-mono text-xs"
        />
      </div>

      <div className="mt-10 flex items-center justify-between gap-4">
        <div className="text-sm text-muted-foreground" aria-live="polite">
          {results.data ? (
            <>
              <span className="font-medium tabular-nums text-foreground">
                {results.data.total.toLocaleString()}
              </span>{" "}
              {results.data.total === 1 ? "result" : "results"}
            </>
          ) : (
            "Searching…"
          )}
        </div>
        {(query || type) && (
          <Button variant="ghost" size="sm" onClick={() => navigate("/search")}>
            Clear filters
          </Button>
        )}
      </div>

      {results.error ? (
        <div className="mt-5 rounded-xl border border-destructive/25 bg-destructive/5 p-5">
          <h2 className="font-medium text-destructive">Search unavailable</h2>
          <p className="mt-1 text-sm text-muted-foreground">
            {(results.error as Error).message}
          </p>
        </div>
      ) : results.isLoading && !results.data ? (
        <div className="mt-5 grid gap-4 md:grid-cols-2 lg:grid-cols-3">
          {Array.from({ length: 9 }).map((_, index) => (
            <Skeleton key={index} className="h-44 rounded-xl" />
          ))}
        </div>
      ) : results.data?.items.length ? (
        <div className="mt-5 grid gap-4 md:grid-cols-2 lg:grid-cols-3">
          {results.data.items.map((hit) => (
            <ObjectResultCard key={hit.uri} hit={hit} />
          ))}
        </div>
      ) : (
        <div className="mt-5 rounded-xl border border-dashed px-6 py-16 text-center">
          <h2 className="font-medium">No matching designs</h2>
          <p className="mt-2 text-sm text-muted-foreground">
            Try a broader term or remove the SBOL type filter.
          </p>
        </div>
      )}

      {results.data && results.data.total > PAGE_SIZE && (
        <nav
          aria-label="Search result pages"
          className="mt-8 flex items-center justify-between border-t pt-6"
        >
          <Button
            variant="outline"
            disabled={offset === 0}
            onClick={() => move(Math.max(0, offset - PAGE_SIZE))}
          >
            <ChevronLeft /> Previous
          </Button>
          <span className="text-xs tabular-nums text-muted-foreground">
            {offset + 1}–
            {Math.min(offset + results.data.items.length, results.data.total)}{" "}
            of {results.data.total.toLocaleString()}
          </span>
          <Button
            variant="outline"
            disabled={offset + PAGE_SIZE >= results.data.total}
            onClick={() => move(offset + PAGE_SIZE)}
          >
            Next <ChevronRight />
          </Button>
        </nav>
      )}
    </div>
  );
}

function positiveInteger(value: string | null): number {
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : 0;
}
