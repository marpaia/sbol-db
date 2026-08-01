import { useMemo, useRef } from "react";
import {
  AlertTriangle,
  ChevronLeft,
  ChevronRight,
  Database,
  Dna,
  Sparkles,
} from "lucide-react";
import { useSearchParams } from "react-router-dom";

import { ObjectSummaryLink } from "@/components/portal/ObjectSummaryLink";
import { Button } from "@/components/ui/button";
import { NativeSelect } from "@/components/ui/native-select";
import { Skeleton } from "@/components/ui/skeleton";
import type {
  StructuredSearchHit,
  StructuredSearchRequest,
  StructuredSearchTotal,
} from "@/features/portal/api";
import { shortIri } from "@/features/portal/format";
import {
  useDiscoveryFacets,
  useStructuredSearch,
} from "@/features/portal/queries";
import type { SearchMethod } from "@/features/portal/search-methods";

type StructuredMethod = Extract<SearchMethod, { kind: "structured" }>;

export function StructuredSearchExperience({
  method,
  query,
}: {
  method: StructuredMethod;
  query: string;
}) {
  const copy = structuredSearchCopy(method.input);
  const [params, setParams] = useSearchParams();
  const cursor = params.get("cursor") || undefined;
  const limit = boundedLimit(params.get("limit"));
  const objectType = params.get("type") || undefined;
  const sequenceExact =
    method.input === "sequence" && params.get("mode") === "exact";
  const explain =
    method.strategy.capabilities.explanations && params.get("explain") === "1";
  const supportsObjectType =
    method.strategy.capabilities.filters.includes("object_type");
  const facets = useDiscoveryFacets(supportsObjectType);
  const cursorParents = useRef(new Map<string, string | undefined>());

  const request = useMemo<StructuredSearchRequest>(
    () => ({
      strategy: method.strategy.id,
      query: structuredQuery(method, query, sequenceExact),
      ...(supportsObjectType && objectType
        ? { filters: { object_types: [objectType] } }
        : {}),
      page: { limit, ...(cursor ? { cursor } : {}) },
      ...(explain ? { options: { explain: true } } : {}),
    }),
    [
      cursor,
      explain,
      limit,
      method,
      objectType,
      query,
      sequenceExact,
      supportsObjectType,
    ]
  );
  const results = useStructuredSearch(request, query.length > 0);

  const setLimit = (nextLimit: number) => {
    const next = new URLSearchParams(params);
    if (nextLimit === 24) next.delete("limit");
    else next.set("limit", String(nextLimit));
    next.delete("cursor");
    cursorParents.current.clear();
    setParams(next);
  };

  const setObjectType = (nextType: string) => {
    const next = new URLSearchParams(params);
    if (nextType) next.set("type", nextType);
    else next.delete("type");
    next.delete("cursor");
    cursorParents.current.clear();
    setParams(next);
  };

  const setExplain = (enabled: boolean) => {
    const next = new URLSearchParams(params);
    if (enabled) next.set("explain", "1");
    else next.delete("explain");
    next.delete("cursor");
    cursorParents.current.clear();
    setParams(next);
  };

  const setSequenceExact = (exact: boolean) => {
    const next = new URLSearchParams(params);
    if (exact) next.set("mode", "exact");
    else next.delete("mode");
    next.delete("cursor");
    cursorParents.current.clear();
    setParams(next);
  };

  const moveNext = (nextCursor: string) => {
    cursorParents.current.set(nextCursor, cursor);
    const next = new URLSearchParams(params);
    next.set("cursor", nextCursor);
    setParams(next);
    window.scrollTo({ top: 0, behavior: "auto" });
  };

  const movePrevious = () => {
    if (!cursor) return;
    const previous = cursorParents.current.get(cursor);
    const next = new URLSearchParams(params);
    if (previous) next.set("cursor", previous);
    else next.delete("cursor");
    setParams(next);
    window.scrollTo({ top: 0, behavior: "auto" });
  };

  const controls = (
    <StructuredSearchControls
      method={method}
      limit={limit}
      objectType={objectType}
      objectTypes={facets.data?.types ?? []}
      objectTypesLoading={facets.isLoading}
      explain={explain}
      sequenceExact={sequenceExact}
      onLimit={setLimit}
      onObjectType={setObjectType}
      onExplain={setExplain}
      onSequenceExact={setSequenceExact}
    />
  );

  return (
    <div className="mt-8 grid gap-8 lg:grid-cols-[260px_minmax(0,1fr)]">
      <aside className="hidden lg:block">
        <div className="sticky top-24 rounded-xl border bg-muted/10 p-5">
          {controls}
        </div>
      </aside>

      <section className="min-w-0" aria-busy={results.isFetching}>
        <div className="mb-5 rounded-xl border bg-muted/10 p-4 lg:hidden">
          {controls}
        </div>

        <div className="flex min-h-10 flex-wrap items-center justify-between gap-3 border-b pb-4">
          <div className="text-sm text-muted-foreground" aria-live="polite">
            {!query ? (
              "Ready for a query"
            ) : results.data ? (
              <>
                {totalLabel(results.data.total, results.data.items.length)}
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
          {results.data?.execution.elapsed_ms !== undefined && (
            <span className="text-xs tabular-nums text-muted-foreground">
              {results.data.execution.elapsed_ms.toLocaleString()} ms
            </span>
          )}
        </div>

        {results.data?.execution.warnings?.map((warning) => (
          <div
            key={warning}
            className="mt-4 flex items-start gap-2 rounded-lg border border-amber-500/25 bg-amber-500/5 px-3 py-2 text-xs leading-5 text-muted-foreground"
          >
            <AlertTriangle className="mt-0.5 size-3.5 shrink-0 text-amber-600 dark:text-amber-300" />
            <span>{warning}</span>
          </div>
        ))}

        {!query ? (
          <div className="mt-5 rounded-xl border border-dashed bg-muted/10 px-6 py-14 text-center">
            {method.input === "sequence" ? (
              <Dna className="mx-auto size-7 text-primary" />
            ) : (
              <Sparkles className="mx-auto size-7 text-primary" />
            )}
            <h2 className="mt-4 font-medium">{copy.emptyTitle}</h2>
            <p className="mx-auto mt-2 max-w-md text-sm leading-6 text-muted-foreground">
              {copy.emptyDescription} The selected method and its settings stay
              in the URL so the search can be shared and reproduced.
            </p>
          </div>
        ) : results.error ? (
          <div
            className="mt-5 rounded-xl border border-destructive/25 bg-destructive/5 p-5"
            role="alert"
          >
            <h2 className="font-medium text-destructive">
              This search method is unavailable
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
          <div className="mt-5 space-y-3" aria-label="Loading search results">
            {Array.from({ length: 5 }).map((_, index) => (
              <Skeleton key={index} className="h-40 rounded-xl" />
            ))}
          </div>
        ) : results.data.items.length > 0 ? (
          <div className="mt-5 space-y-3">
            {results.data.items.map((hit) => (
              <ObjectSummaryLink
                key={hit.document_id}
                object={structuredSummary(hit)}
                metadata={<StructuredHitMetadata hit={hit} />}
              />
            ))}
          </div>
        ) : (
          <div className="mt-5 rounded-xl border border-dashed bg-muted/10 px-6 py-14 text-center">
            <h2 className="font-medium">No matching designs</h2>
            <p className="mx-auto mt-2 max-w-md text-sm leading-6 text-muted-foreground">
              Try a broader description, remove the object type filter, or use
              another search method.
            </p>
          </div>
        )}

        {results.data && (cursor || results.data.next_cursor) && (
          <nav
            aria-label="Search result pages"
            className="mt-8 flex items-center justify-between gap-4 border-t pt-6"
          >
            <Button
              variant="outline"
              disabled={!cursor || !cursorParents.current.has(cursor)}
              onClick={movePrevious}
            >
              <ChevronLeft /> <span className="hidden sm:inline">Previous</span>
            </Button>
            <span className="text-center text-xs text-muted-foreground">
              Cursor page
            </span>
            <Button
              variant="outline"
              disabled={!results.data.next_cursor}
              onClick={() => {
                if (results.data?.next_cursor)
                  moveNext(results.data.next_cursor);
              }}
            >
              <span className="hidden sm:inline">Next</span> <ChevronRight />
            </Button>
          </nav>
        )}
      </section>
    </div>
  );
}

function StructuredSearchControls({
  method,
  limit,
  objectType,
  objectTypes,
  objectTypesLoading,
  explain,
  sequenceExact,
  onLimit,
  onObjectType,
  onExplain,
  onSequenceExact,
}: {
  method: StructuredMethod;
  limit: number;
  objectType?: string;
  objectTypes: Array<{ iri: string; label: string; count: number }>;
  objectTypesLoading: boolean;
  explain: boolean;
  sequenceExact: boolean;
  onLimit: (limit: number) => void;
  onObjectType: (objectType: string) => void;
  onExplain: (explain: boolean) => void;
  onSequenceExact: (exact: boolean) => void;
}) {
  const { capabilities } = method.strategy;
  const copy = structuredSearchCopy(method.input);
  return (
    <div>
      <div className="flex items-start gap-3">
        <Database className="mt-0.5 size-4 shrink-0 text-primary" />
        <div className="min-w-0">
          <h2 className="text-sm font-semibold">{copy.title}</h2>
          <p className="mt-1 text-xs leading-5 text-muted-foreground">
            {copy.sidebarDescription}
          </p>
        </div>
      </div>

      {method.input === "sequence" && (
        <fieldset className="mt-5">
          <legend className="text-xs font-medium">Sequence query</legend>
          <div className="mt-2 grid gap-1 rounded-lg border bg-background p-1">
            <Button
              type="button"
              size="sm"
              variant={!sequenceExact ? "secondary" : "ghost"}
              className="h-auto justify-start px-3 py-2 text-left"
              aria-pressed={!sequenceExact}
              onClick={() => onSequenceExact(false)}
            >
              Ranked similarity
            </Button>
            <Button
              type="button"
              size="sm"
              variant={sequenceExact ? "secondary" : "ghost"}
              className="h-auto justify-start px-3 py-2 text-left"
              aria-pressed={sequenceExact}
              onClick={() => onSequenceExact(true)}
            >
              Exact match
            </Button>
          </div>
        </fieldset>
      )}

      {capabilities.filters.includes("object_type") && (
        <div className="mt-5">
          <label
            htmlFor="structured-object-type"
            className="text-xs font-medium"
          >
            Object type
          </label>
          <NativeSelect
            id="structured-object-type"
            value={objectType || ""}
            disabled={objectTypesLoading}
            onChange={(event) => onObjectType(event.target.value)}
            className="mt-2 w-full"
          >
            <option value="">All object types</option>
            {objectTypes.map((type) => (
              <option key={type.iri} value={type.iri}>
                {type.label} ({type.count.toLocaleString()})
              </option>
            ))}
          </NativeSelect>
        </div>
      )}

      <div className="mt-5">
        <label htmlFor="structured-limit" className="text-xs font-medium">
          Results per page
        </label>
        <NativeSelect
          id="structured-limit"
          value={limit}
          onChange={(event) => onLimit(Number(event.target.value))}
          className="mt-2 w-full"
        >
          {[12, 24, 48, 100].map((value) => (
            <option key={value} value={value}>
              {value}
            </option>
          ))}
          {![12, 24, 48, 100].includes(limit) && (
            <option value={limit}>{limit}</option>
          )}
        </NativeSelect>
      </div>

      {capabilities.explanations && (
        <label className="mt-5 flex cursor-pointer items-start gap-2.5 text-xs leading-5">
          <input
            type="checkbox"
            checked={explain}
            onChange={(event) => onExplain(event.target.checked)}
            className="mt-0.5 size-4 rounded border-input accent-primary"
          />
          <span>
            Include match evidence
            <span className="block text-muted-foreground">
              Show evidence supplied by this strategy.
            </span>
          </span>
        </label>
      )}

      {capabilities.data_egress === "configured_remote" && (
        <p className="mt-4 text-xs leading-5 text-amber-700 dark:text-amber-300">
          This search method may send the query to a configured remote service.
        </p>
      )}
    </div>
  );
}

function StructuredHitMetadata({ hit }: { hit: StructuredSearchHit }) {
  const sources = Array.from(
    new Set(hit.evidence?.map((evidence) => evidence.source) ?? [])
  );
  return (
    <>
      <span className="rounded-full bg-primary/10 px-2.5 py-0.5 text-xs font-semibold tabular-nums text-primary">
        Score {hit.score.toFixed(3)}
      </span>
      {sources.map((source) => (
        <span
          key={source}
          className="font-mono text-[10px] text-muted-foreground"
        >
          {shortIri(source)}
        </span>
      ))}
    </>
  );
}

function structuredSummary(hit: StructuredSearchHit) {
  return {
    uri: hit.uri,
    display_id: hit.display_id ?? null,
    name: hit.name ?? null,
    description: hit.description ?? null,
    object_type: hit.object_types[0] ?? null,
  };
}

function structuredQuery(
  method: StructuredMethod,
  query: string,
  sequenceExact: boolean
) {
  if (method.input === "sequence") {
    return {
      kind: "sequence" as const,
      sequence: query.replace(/\s+/g, "").toUpperCase(),
      exact: sequenceExact,
    };
  }
  if (method.input === "similar") {
    return { kind: "similar" as const, uri: query };
  }
  return { kind: "text" as const, text: query };
}

function totalLabel(total: StructuredSearchTotal, pageSize: number) {
  if (total.kind === "exact") {
    return (
      <>
        <span className="font-semibold tabular-nums text-foreground">
          {total.value.toLocaleString()}
        </span>{" "}
        {total.value === 1 ? "design" : "designs"}
      </>
    );
  }
  if (total.kind === "lower_bound") {
    return (
      <>
        At least{" "}
        <span className="font-semibold tabular-nums text-foreground">
          {total.value.toLocaleString()}
        </span>{" "}
        designs
      </>
    );
  }
  return (
    <>
      <span className="font-semibold tabular-nums text-foreground">
        {pageSize.toLocaleString()}
      </span>{" "}
      {pageSize === 1 ? "design on this page" : "designs on this page"}
    </>
  );
}

function boundedLimit(value: string | null): number {
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed >= 1 && parsed <= 1000
    ? parsed
    : 24;
}

function structuredSearchCopy(input: StructuredMethod["input"]) {
  if (input === "similar") {
    return {
      title: "Related-design search",
      sidebarDescription: "Find registry objects related to a known design.",
      emptyTitle: "Find related designs",
      emptyDescription:
        "Enter the URI of a registry design to use as the starting point.",
    };
  }
  if (input === "sequence") {
    return {
      title: "Specialized sequence search",
      sidebarDescription: "Compare DNA with a configured sequence method.",
      emptyTitle: "Search by DNA sequence",
      emptyDescription:
        "Enter a nucleotide sequence to find biologically similar designs.",
    };
  }
  return {
    title: "Biological meaning",
    sidebarDescription:
      "Find designs by function or concept, not only exact words.",
    emptyTitle: "Describe the biology you need",
    emptyDescription:
      "Search for a function, biological concept, part, or intended behavior.",
  };
}
