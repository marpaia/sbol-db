import { useEffect, useMemo, useState } from "react";
import {
  ArrowDownNarrowWide,
  ArrowUpNarrowWide,
  ChevronLeft,
  ChevronRight,
  Grid2X2,
  List,
  SlidersHorizontal,
} from "lucide-react";
import { Navigate, useParams, useSearchParams } from "react-router-dom";

import {
  DiscoveryFilters,
  DiscoveryFilterSummary,
  type DiscoveryFilterKey,
} from "@/components/portal/DiscoveryFilters";
import { ObjectResultCard } from "@/components/portal/ObjectResultCard";
import { SearchCompatibilityNotice } from "@/components/portal/SearchCompatibilityNotice";
import { UnifiedSearchInput } from "@/components/portal/UnifiedSearchInput";
import { Button } from "@/components/ui/button";
import { NativeSelect } from "@/components/ui/native-select";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import type {
  DiscoverySort,
  PortalSearchQuery,
  SortDirection,
} from "@/features/portal/api";
import {
  hasDiscoveryFilters,
  naturalDirection,
  parseDiscoveryParams,
  translateClassicSearchPath,
} from "@/features/portal/discovery";
import {
  useDiscoveryFacets,
  useInstance,
  usePortalSearch,
  useSearchStrategies,
} from "@/features/portal/queries";
import {
  activeSearchMethod,
  buildSearchMethods,
  paramsForSearchMethod,
} from "@/features/portal/search-methods";
import { cn } from "@/lib/utils";
import { SequenceSearchExperience } from "@/routes/SequenceSearchRoute";
import { StructuredSearchExperience } from "@/routes/StructuredSearchExperience";

const FILTER_PARAM: Record<DiscoveryFilterKey, string> = {
  q: "q",
  type: "type",
  role: "role",
  collection: "collection",
  owner: "owner",
  provenance: "provenance",
  createdAfter: "created_after",
  createdBefore: "created_before",
  modifiedAfter: "modified_after",
  modifiedBefore: "modified_before",
};

const FILTER_KEYS = Object.values(FILTER_PARAM);

export default function SearchRoute() {
  const classicPath = useParams()["*"]?.trim() || "";
  const classicLocation = useMemo(
    () => (classicPath ? translateClassicSearchPath(classicPath) : null),
    [classicPath]
  );
  return classicPath ? (
    <Navigate
      to={{
        pathname: classicLocation?.pathname || "/search",
        search: classicLocation?.params.toString() || "",
      }}
      replace
    />
  ) : (
    <SearchExperience />
  );
}

function SearchExperience() {
  const [params, setParams] = useSearchParams();
  const strategies = useSearchStrategies();
  const instance = useInstance();
  const methods = useMemo(
    () =>
      buildSearchMethods(
        strategies.data,
        instance.data?.capabilities.sequence_search !== false
      ),
    [instance.data?.capabilities.sequence_search, strategies.data]
  );
  const method = activeSearchMethod(params, methods);
  const query = params.get("q") || "";
  const requestedStrategy = params.get("strategy");
  const selectedStrategyMissing =
    !strategies.isLoading && requestedStrategy && method.kind !== "structured";

  const selectMethod = (nextMethod: (typeof methods)[number]) => {
    setParams(paramsForSearchMethod(params, nextMethod));
  };

  const submit = (value: string) => {
    const next = new URLSearchParams(params);
    const normalized =
      method.input === "sequence"
        ? value.replace(/\s+/g, "").toUpperCase()
        : value.trim();
    if (normalized) next.set("q", normalized);
    else next.delete("q");
    next.delete("offset");
    next.delete("cursor");
    setParams(next);
  };

  return (
    <div className="mx-auto w-full max-w-7xl px-4 py-8 sm:px-6 sm:py-10 lg:px-8">
      <header className="max-w-4xl">
        <p className="text-xs font-medium uppercase tracking-[0.16em] text-primary">
          Registry search
        </p>
        <h1 className="mt-2 text-3xl font-semibold tracking-[-0.025em] sm:text-4xl">
          Search biological designs
        </h1>
        <p className="mt-2 max-w-2xl text-sm leading-6 text-muted-foreground">
          Search the full corpus visible to your account by design information,
          biological meaning, or DNA sequence.
        </p>
      </header>

      <UnifiedSearchInput
        methods={methods}
        method={method}
        query={query}
        strategiesLoading={strategies.isLoading}
        strategiesError={
          selectedStrategyMissing
            ? `The requested index “${requestedStrategy}” is not configured.`
            : strategies.error instanceof Error
              ? strategies.error.message
              : undefined
        }
        onMethodChange={selectMethod}
        onSearch={submit}
      />

      {method.kind === "sequence" ? (
        <SequenceSearchExperience />
      ) : method.kind === "structured" ? (
        <StructuredSearchExperience method={method} query={query} />
      ) : (
        <DiscoverySearch />
      )}
    </div>
  );
}

function DiscoverySearch() {
  const [params, setParams] = useSearchParams();
  const [filtersOpen, setFiltersOpen] = useState(false);
  const state = useMemo(() => parseDiscoveryParams(params), [params]);
  const results = usePortalSearch(state.query);
  const facets = useDiscoveryFacets();
  const { offset, limit, sort, direction } = state.query;

  useEffect(() => {
    if (
      results.data &&
      results.data.total > 0 &&
      offset >= results.data.total
    ) {
      const next = new URLSearchParams(params);
      const lastOffset = Math.floor((results.data.total - 1) / limit) * limit;
      setNumberParam(next, "offset", lastOffset, 0);
      setParams(next, { replace: true });
    }
  }, [limit, offset, params, results.data, setParams]);

  const updateFilters = (
    changes: Partial<Record<DiscoveryFilterKey, string | undefined>>
  ) => {
    const next = new URLSearchParams(params);
    for (const [key, value] of Object.entries(changes)) {
      const param = FILTER_PARAM[key as DiscoveryFilterKey];
      if (value?.trim()) next.set(param, value.trim());
      else next.delete(param);
    }
    next.delete("offset");
    setParams(next);
  };

  const clearFilters = () => {
    const next = new URLSearchParams(params);
    FILTER_KEYS.forEach((key) => next.delete(key));
    next.delete("offset");
    next.delete("compat");
    next.delete("compat_warning");
    setParams(next);
  };

  const dismissCompatibilityNotice = () => {
    const next = new URLSearchParams(params);
    next.delete("compat");
    next.delete("compat_warning");
    setParams(next, { replace: true });
  };

  const setSort = (nextSort: DiscoverySort) => {
    const next = new URLSearchParams(params);
    setStringParam(next, "sort", nextSort, "relevance");
    setStringParam(
      next,
      "direction",
      naturalDirection(nextSort),
      naturalDirection(nextSort)
    );
    next.delete("offset");
    setParams(next);
  };

  const setDirection = (nextDirection: SortDirection) => {
    const next = new URLSearchParams(params);
    setStringParam(next, "direction", nextDirection, naturalDirection(sort));
    next.delete("offset");
    setParams(next);
  };

  const setLimit = (nextLimit: number) => {
    const next = new URLSearchParams(params);
    setNumberParam(next, "limit", nextLimit, 24);
    next.delete("offset");
    setParams(next);
  };

  const setView = (view: "grid" | "list") => {
    const next = new URLSearchParams(params);
    setStringParam(next, "view", view, "grid");
    setParams(next, { replace: true });
  };

  const move = (nextOffset: number) => {
    const next = new URLSearchParams(params);
    setNumberParam(next, "offset", nextOffset, 0);
    setParams(next);
    window.scrollTo({ top: 0, behavior: "auto" });
  };

  const activeFilterCount = FILTER_KEYS.filter((key) => params.has(key)).length;
  const filtersProps = {
    query: state.query as PortalSearchQuery,
    facets: facets.data,
    facetsLoading: facets.isLoading,
    facetsError:
      facets.error instanceof Error ? facets.error.message : undefined,
    onChange: updateFilters,
    onClear: clearFilters,
  };

  return (
    <>
      {state.translatedFromClassic && (
        <SearchCompatibilityNotice
          warnings={state.compatibilityWarnings}
          onDismiss={dismissCompatibilityNotice}
        />
      )}

      <div className="mt-8 grid gap-8 lg:grid-cols-[260px_minmax(0,1fr)]">
        <aside className="hidden lg:block">
          <div className="sticky top-24 rounded-xl border bg-muted/10 p-5">
            <DiscoveryFilters {...filtersProps} />
          </div>
        </aside>

        <section className="min-w-0" aria-busy={results.isFetching}>
          <div className="flex flex-wrap items-center justify-between gap-3 border-b pb-4">
            <div className="flex items-center gap-3">
              <Sheet open={filtersOpen} onOpenChange={setFiltersOpen}>
                <SheetTrigger asChild>
                  <Button variant="outline" className="lg:hidden">
                    <SlidersHorizontal /> Filters
                    {activeFilterCount > 0 && (
                      <span className="rounded-full bg-primary px-1.5 py-0.5 text-[10px] leading-none text-primary-foreground">
                        {activeFilterCount}
                      </span>
                    )}
                  </Button>
                </SheetTrigger>
                <SheetContent
                  side="left"
                  className="w-[min(92vw,24rem)] overflow-y-auto sm:max-w-sm"
                >
                  <SheetHeader className="mb-6 pr-8">
                    <SheetTitle>Discovery filters</SheetTitle>
                    <SheetDescription>
                      Narrow the visible registry without losing your place.
                    </SheetDescription>
                  </SheetHeader>
                  <DiscoveryFilters
                    {...filtersProps}
                    onApplied={() => setFiltersOpen(false)}
                  />
                </SheetContent>
              </Sheet>

              <div className="text-sm text-muted-foreground" aria-live="polite">
                {results.data ? (
                  <>
                    <span className="font-semibold tabular-nums text-foreground">
                      {results.data.total.toLocaleString()}
                    </span>{" "}
                    {results.data.total === 1 ? "design" : "designs"}
                    {results.isFetching && (
                      <span className="ml-2 text-xs">Updating…</span>
                    )}
                  </>
                ) : results.isLoading ? (
                  "Searching…"
                ) : (
                  "No result count"
                )}
              </div>
            </div>

            <DiscoveryToolbar
              sort={sort}
              direction={direction}
              limit={limit}
              view={state.view}
              onSort={setSort}
              onDirection={setDirection}
              onLimit={setLimit}
              onView={setView}
            />
          </div>

          {hasDiscoveryFilters(state.query) && (
            <div className="pt-4">
              <DiscoveryFilterSummary
                query={state.query}
                facets={facets.data}
                onRemove={(key) => updateFilters({ [key]: undefined })}
              />
            </div>
          )}

          {results.error ? (
            <div
              className="mt-5 rounded-xl border border-destructive/25 bg-destructive/5 p-5"
              role="alert"
            >
              <h2 className="font-medium text-destructive">
                Discovery is unavailable
              </h2>
              <p className="mt-1 text-sm leading-6 text-muted-foreground">
                {(results.error as Error).message}
              </p>
              <Button
                variant="outline"
                size="sm"
                className="mt-4"
                onClick={() => results.refetch()}
              >
                Try again
              </Button>
            </div>
          ) : results.isLoading || !results.data ? (
            <ResultSkeleton view={state.view} />
          ) : results.data.items.length ? (
            <div
              className={cn(
                "mt-5",
                state.view === "grid"
                  ? "grid gap-4 md:grid-cols-2 xl:grid-cols-3"
                  : "space-y-3"
              )}
            >
              {results.data.items.map((hit) => (
                <ObjectResultCard
                  key={hit.uri}
                  hit={hit}
                  variant={state.view === "grid" ? "card" : "row"}
                />
              ))}
            </div>
          ) : (
            <div className="mt-5 rounded-xl border border-dashed bg-muted/10 px-6 py-16 text-center">
              <h2 className="font-medium">No matching designs</h2>
              <p className="mx-auto mt-2 max-w-md text-sm leading-6 text-muted-foreground">
                No visible object satisfies every active filter. Remove one or
                search with a broader term.
              </p>
              {hasDiscoveryFilters(state.query) && (
                <Button
                  variant="outline"
                  className="mt-5"
                  onClick={clearFilters}
                >
                  Clear all filters
                </Button>
              )}
            </div>
          )}

          {results.data && results.data.total > limit && (
            <Pagination
              offset={offset}
              limit={limit}
              total={results.data.total}
              itemCount={results.data.items.length}
              onMove={move}
            />
          )}
        </section>
      </div>
    </>
  );
}

function DiscoveryToolbar({
  sort,
  direction,
  limit,
  view,
  onSort,
  onDirection,
  onLimit,
  onView,
}: {
  sort: DiscoverySort;
  direction: SortDirection;
  limit: number;
  view: "grid" | "list";
  onSort: (sort: DiscoverySort) => void;
  onDirection: (direction: SortDirection) => void;
  onLimit: (limit: number) => void;
  onView: (view: "grid" | "list") => void;
}) {
  const DirectionIcon =
    direction === "asc" ? ArrowUpNarrowWide : ArrowDownNarrowWide;
  return (
    <div className="flex items-center gap-1.5">
      <label htmlFor="discovery-sort" className="sr-only">
        Sort results
      </label>
      <NativeSelect
        id="discovery-sort"
        value={sort}
        onChange={(event) => onSort(event.target.value as DiscoverySort)}
        className="w-32 sm:w-36"
      >
        <option value="relevance">Relevance</option>
        <option value="name">Name</option>
        <option value="created">Created</option>
        <option value="modified">Modified</option>
        <option value="iri">IRI</option>
      </NativeSelect>
      <Button
        variant="outline"
        size="icon"
        onClick={() => onDirection(direction === "asc" ? "desc" : "asc")}
        aria-label={`Sort ${direction === "asc" ? "descending" : "ascending"}`}
        title={`Currently ${direction === "asc" ? "ascending" : "descending"}`}
      >
        <DirectionIcon />
      </Button>

      <label htmlFor="discovery-limit" className="sr-only">
        Results per page
      </label>
      <NativeSelect
        id="discovery-limit"
        value={limit}
        onChange={(event) => onLimit(Number(event.target.value))}
        className="hidden w-20 sm:block"
        title="Results per page"
      >
        {[12, 24, 48, 100].map((size) => (
          <option key={size} value={size}>
            {size}
          </option>
        ))}
        {![12, 24, 48, 100].includes(limit) && (
          <option value={limit}>{limit}</option>
        )}
      </NativeSelect>

      <div className="ml-1 hidden rounded-md border p-0.5 sm:flex">
        <Button
          variant={view === "grid" ? "secondary" : "ghost"}
          size="icon"
          className="size-7"
          aria-label="Grid view"
          aria-pressed={view === "grid"}
          onClick={() => onView("grid")}
        >
          <Grid2X2 />
        </Button>
        <Button
          variant={view === "list" ? "secondary" : "ghost"}
          size="icon"
          className="size-7"
          aria-label="List view"
          aria-pressed={view === "list"}
          onClick={() => onView("list")}
        >
          <List />
        </Button>
      </div>
    </div>
  );
}

function ResultSkeleton({ view }: { view: "grid" | "list" }) {
  return (
    <div
      className={cn(
        "mt-5",
        view === "grid"
          ? "grid gap-4 md:grid-cols-2 xl:grid-cols-3"
          : "space-y-3"
      )}
      aria-label="Loading search results"
    >
      {Array.from({ length: view === "grid" ? 9 : 6 }).map((_, index) => (
        <Skeleton
          key={index}
          className={cn("rounded-xl", view === "grid" ? "h-52" : "h-40")}
        />
      ))}
    </div>
  );
}

function Pagination({
  offset,
  limit,
  total,
  itemCount,
  onMove,
}: {
  offset: number;
  limit: number;
  total: number;
  itemCount: number;
  onMove: (offset: number) => void;
}) {
  const page = Math.floor(offset / limit) + 1;
  const pages = Math.ceil(total / limit);
  return (
    <nav
      aria-label="Search result pages"
      className="mt-8 flex items-center justify-between gap-4 border-t pt-6"
    >
      <Button
        variant="outline"
        disabled={offset === 0}
        onClick={() => onMove(Math.max(0, offset - limit))}
      >
        <ChevronLeft /> <span className="hidden sm:inline">Previous</span>
      </Button>
      <span className="text-center text-xs tabular-nums text-muted-foreground">
        <span className="hidden sm:inline">
          {offset + 1}–{Math.min(offset + itemCount, total)} of{" "}
          {total.toLocaleString()} ·{" "}
        </span>
        Page {page.toLocaleString()} of {pages.toLocaleString()}
      </span>
      <Button
        variant="outline"
        disabled={offset + limit >= total}
        onClick={() => onMove(offset + limit)}
      >
        <span className="hidden sm:inline">Next</span> <ChevronRight />
      </Button>
    </nav>
  );
}

function setStringParam(
  params: URLSearchParams,
  key: string,
  value: string,
  defaultValue: string
) {
  if (value === defaultValue) params.delete(key);
  else params.set(key, value);
}

function setNumberParam(
  params: URLSearchParams,
  key: string,
  value: number,
  defaultValue: number
) {
  if (value === defaultValue) params.delete(key);
  else params.set(key, String(value));
}
