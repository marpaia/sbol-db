import { Dna } from "lucide-react";
import { Navigate, useSearchParams } from "react-router-dom";

import { ObjectSummaryLink } from "@/components/portal/ObjectSummaryLink";
import { SearchCompatibilityNotice } from "@/components/portal/SearchCompatibilityNotice";
import { Button } from "@/components/ui/button";
import { NativeSelect } from "@/components/ui/native-select";
import { Skeleton } from "@/components/ui/skeleton";
import { canonicalSequenceSearchParams } from "@/features/registry/discovery/discovery";
import { useSequenceSearch } from "@/features/registry/discovery/queries";

/** Preserve SynBioHub's public URL while making `/search` the canonical page. */
export default function SequenceSearchRoute() {
  const [params] = useSearchParams();
  const canonical = canonicalSequenceSearchParams(params);

  return (
    <Navigate
      to={{ pathname: "/search", search: canonical.toString() }}
      replace
    />
  );
}

export function SequenceSearchExperience() {
  const [params, setParams] = useSearchParams();
  const query = normalizeSequence(params.get("q") || "");
  const mode = params.get("mode") === "exact" ? "exact" : "global";
  const limit = boundedLimit(params.get("limit"));
  const results = useSequenceSearch({ q: query, mode, limit });
  const translatedFromClassic = params.get("compat") === "classic";
  const warnings = params.getAll("compat_warning");

  const setMode = (nextMode: "global" | "exact") => {
    const next = new URLSearchParams(params);
    if (nextMode === "global") next.delete("mode");
    else next.set("mode", nextMode);
    setParams(next);
  };

  const setLimit = (nextLimit: number) => {
    const next = new URLSearchParams(params);
    if (nextLimit === 50) next.delete("limit");
    else next.set("limit", String(nextLimit));
    setParams(next);
  };

  const dismissCompatibilityNotice = () => {
    const next = new URLSearchParams(params);
    next.delete("compat");
    next.delete("compat_warning");
    setParams(next, { replace: true });
  };

  const controls = (
    <SequenceControls
      mode={mode}
      limit={limit}
      onMode={setMode}
      onLimit={setLimit}
    />
  );

  return (
    <>
      {translatedFromClassic && (
        <SearchCompatibilityNotice
          warnings={warnings}
          onDismiss={dismissCompatibilityNotice}
        />
      )}

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

          <div className="flex min-h-10 items-center justify-between gap-3 border-b pb-4">
            <div className="text-sm text-muted-foreground" aria-live="polite">
              {!query ? (
                "Ready for a sequence query"
              ) : results.data ? (
                <>
                  <span className="font-semibold tabular-nums text-foreground">
                    {results.data.total.toLocaleString()}
                  </span>{" "}
                  {results.data.total === 1
                    ? "sequence match"
                    : "sequence matches"}
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
            <span className="hidden text-xs text-muted-foreground sm:block">
              {mode === "global" ? "Ranked alignment" : "Exact substring"}
            </span>
          </div>

          {!query ? (
            <div className="mt-5 rounded-xl border border-dashed bg-muted/10 px-6 py-14 text-center">
              <Dna className="mx-auto size-7 text-primary" />
              <h2 className="mt-4 font-medium">Enter a nucleotide sequence</h2>
              <p className="mx-auto mt-2 max-w-md text-sm leading-6 text-muted-foreground">
                Results remain scoped to designs visible to your account. The
                sequence and matching behavior stay in the URL so this search
                can be shared and reproduced.
              </p>
            </div>
          ) : results.error ? (
            <div
              className="mt-5 rounded-xl border border-destructive/25 bg-destructive/5 p-5"
              role="alert"
            >
              <h2 className="font-medium text-destructive">
                Sequence search is unavailable
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
            <div
              className="mt-5 space-y-3"
              aria-label="Loading sequence matches"
            >
              {Array.from({ length: 5 }).map((_, index) => (
                <Skeleton key={index} className="h-40 rounded-xl" />
              ))}
            </div>
          ) : results.data.items.length > 0 ? (
            <div className="mt-5 space-y-3">
              {results.data.items.map((hit) => (
                <ObjectSummaryLink
                  key={hit.uri}
                  object={hit}
                  metadata={
                    <>
                      <span className="border border-primary/20 bg-primary/5 px-2.5 py-0.5 text-xs font-semibold tabular-nums text-primary">
                        {(hit.percent_match * 100).toFixed(1)}% identity
                      </span>
                      <span className="font-mono text-[11px] text-muted-foreground">
                        {strandLabel(hit.strand)} · {hit.cigar}
                      </span>
                    </>
                  }
                />
              ))}
            </div>
          ) : (
            <div className="mt-5 rounded-xl border border-dashed bg-muted/10 px-6 py-14 text-center">
              <h2 className="font-medium">No sequence matches</h2>
              <p className="mx-auto mt-2 max-w-md text-sm leading-6 text-muted-foreground">
                Try exact mode for a known motif or a longer, more
                representative sequence in global mode.
              </p>
            </div>
          )}
        </section>
      </div>
    </>
  );
}

function SequenceControls({
  mode,
  limit,
  onMode,
  onLimit,
}: {
  mode: "global" | "exact";
  limit: number;
  onMode: (mode: "global" | "exact") => void;
  onLimit: (limit: number) => void;
}) {
  return (
    <div>
      <div>
        <h2 className="text-sm font-semibold">Match settings</h2>
        <p className="mt-1 text-xs leading-5 text-muted-foreground">
          Choose how the query should be compared with visible registry
          sequences.
        </p>
      </div>

      <fieldset className="mt-5">
        <legend className="text-xs font-medium">Match behavior</legend>
        <div className="mt-2 grid gap-1 rounded-lg border bg-background p-1">
          <Button
            type="button"
            size="sm"
            variant={mode === "global" ? "secondary" : "ghost"}
            className="h-auto justify-start px-3 py-2 text-left"
            aria-pressed={mode === "global"}
            onClick={() => onMode("global")}
          >
            Global alignment
          </Button>
          <Button
            type="button"
            size="sm"
            variant={mode === "exact" ? "secondary" : "ghost"}
            className="h-auto justify-start px-3 py-2 text-left"
            aria-pressed={mode === "exact"}
            onClick={() => onMode("exact")}
          >
            Exact substring
          </Button>
        </div>
      </fieldset>

      <div className="mt-5">
        <label htmlFor="sequence-limit" className="text-xs font-medium">
          Maximum results
        </label>
        <NativeSelect
          id="sequence-limit"
          value={limit}
          onChange={(event) => onLimit(Number(event.target.value))}
          className="mt-2 w-full"
        >
          {[25, 50, 100, 250].map((value) => (
            <option key={value} value={value}>
              {value}
            </option>
          ))}
          {![25, 50, 100, 250].includes(limit) && (
            <option value={limit}>{limit}</option>
          )}
        </NativeSelect>
      </div>

      <p className="mt-5 border-t pt-4 text-xs leading-5 text-muted-foreground">
        {mode === "global"
          ? "Ranks sufficiently similar sequences by alignment identity and registry rank."
          : "Finds the query as a contiguous substring on either DNA strand."}
      </p>
    </div>
  );
}

function normalizeSequence(value: string): string {
  return value.replace(/\s+/g, "").toUpperCase();
}

function boundedLimit(value: string | null): number {
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed >= 1 && parsed <= 1000
    ? parsed
    : 50;
}

function strandLabel(value: string): string {
  if (value === "+" || value.toLowerCase() === "forward") return "Forward";
  if (value === "-" || value.toLowerCase() === "reverse") return "Reverse";
  return value;
}
