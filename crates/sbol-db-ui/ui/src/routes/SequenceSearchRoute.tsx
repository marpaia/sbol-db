import { useEffect, useState } from "react";
import { AlertTriangle, Dna, Search } from "lucide-react";
import { useSearchParams } from "react-router-dom";

import { DiscoveryModeNav } from "@/components/portal/DiscoveryModeNav";
import { ObjectSummaryLink } from "@/components/portal/ObjectSummaryLink";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { NativeSelect } from "@/components/ui/native-select";
import { Skeleton } from "@/components/ui/skeleton";
import { useSequenceSearch } from "@/features/portal/queries";
import { cn } from "@/lib/utils";

const EXAMPLES = ["GAATTC", "GGTACC", "AAGCTT"];

export default function SequenceSearchRoute() {
  const [params, setParams] = useSearchParams();
  const query = normalizeSequence(params.get("q") || "");
  const mode = params.get("mode") === "exact" ? "exact" : "global";
  const limit = boundedLimit(params.get("limit"));
  const [draft, setDraft] = useState(query);
  const results = useSequenceSearch({ q: query, mode, limit });
  const translatedFromClassic = params.get("compat") === "classic";
  const warnings = params.getAll("compat_warning");

  useEffect(() => setDraft(query), [query]);

  const submit = (sequence: string) => {
    const next = new URLSearchParams(params);
    const normalized = normalizeSequence(sequence);
    if (normalized) next.set("q", normalized);
    else next.delete("q");
    setParams(next);
  };

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

  return (
    <div className="mx-auto w-full max-w-6xl px-4 py-8 sm:px-6 sm:py-10 lg:px-8">
      <header className="max-w-4xl">
        <p className="text-xs font-medium uppercase tracking-[0.16em] text-primary">
          Registry discovery
        </p>
        <h1 className="mt-2 text-3xl font-semibold tracking-[-0.025em] sm:text-4xl">
          Search by DNA sequence
        </h1>
        <p className="mt-2 max-w-2xl text-sm leading-6 text-muted-foreground">
          Find visible SBOL sequences by exact substring or ranked global
          alignment, including reverse-complement matches.
        </p>
        <DiscoveryModeNav />
      </header>

      <form
        role="search"
        className="mt-6 rounded-xl border bg-card p-4 shadow-sm sm:p-5"
        onSubmit={(event) => {
          event.preventDefault();
          submit(draft);
        }}
      >
        <label htmlFor="sequence-query" className="text-xs font-medium">
          Nucleotide sequence
        </label>
        <div className="mt-2 flex items-center rounded-lg border bg-background p-1.5 focus-within:border-primary/50 focus-within:ring-4 focus-within:ring-primary/10">
          <Dna className="ml-2 size-4 shrink-0 text-muted-foreground" />
          <Input
            id="sequence-query"
            value={draft}
            onChange={(event) => setDraft(event.target.value.toUpperCase())}
            placeholder="Enter DNA using IUPAC nucleotide codes…"
            autoComplete="off"
            autoCapitalize="characters"
            spellCheck={false}
            className="h-10 border-0 bg-transparent font-mono shadow-none focus-visible:ring-0"
          />
          <Button type="submit" disabled={!normalizeSequence(draft)}>
            <Search /> Search
          </Button>
        </div>

        <div className="mt-4 flex flex-wrap items-center gap-4">
          <div
            className="inline-flex rounded-md border p-0.5"
            aria-label="Sequence matching mode"
          >
            <Button
              type="button"
              size="sm"
              variant={mode === "global" ? "secondary" : "ghost"}
              aria-pressed={mode === "global"}
              onClick={() => setMode("global")}
            >
              Global alignment
            </Button>
            <Button
              type="button"
              size="sm"
              variant={mode === "exact" ? "secondary" : "ghost"}
              aria-pressed={mode === "exact"}
              onClick={() => setMode("exact")}
            >
              Exact substring
            </Button>
          </div>

          <div className="flex items-center gap-2">
            <label
              htmlFor="sequence-limit"
              className="text-xs text-muted-foreground"
            >
              Max results
            </label>
            <NativeSelect
              id="sequence-limit"
              value={limit}
              onChange={(event) => setLimit(Number(event.target.value))}
              className="w-20"
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

          <div className="flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground sm:ml-auto">
            <span>Examples</span>
            {EXAMPLES.map((example) => (
              <button
                key={example}
                type="button"
                className="rounded-md border bg-background px-2 py-1 font-mono text-[11px] text-foreground transition-colors duration-150 hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring active:bg-primary/10"
                onClick={() => {
                  setDraft(example);
                  submit(example);
                }}
              >
                {example}
              </button>
            ))}
          </div>
        </div>

        <p className="mt-3 text-xs leading-5 text-muted-foreground">
          {mode === "global"
            ? "Global mode ranks sufficiently similar sequences by alignment identity and registry rank."
            : "Exact mode finds the query as a contiguous substring on either strand."}
        </p>
      </form>

      {translatedFromClassic && (
        <div
          className={cn(
            "mt-5 flex items-start gap-3 rounded-xl border p-4 text-sm",
            warnings.length
              ? "border-amber-500/30 bg-amber-500/5"
              : "border-primary/20 bg-primary/5"
          )}
          role={warnings.length ? "alert" : "status"}
        >
          <AlertTriangle
            className={cn(
              "mt-0.5 size-4 shrink-0",
              warnings.length
                ? "text-amber-600 dark:text-amber-400"
                : "text-primary"
            )}
          />
          <div className="min-w-0 flex-1">
            <p className="font-medium">
              Compatibility sequence link translated
            </p>
            {warnings.length > 0 && (
              <ul className="mt-1 list-disc space-y-1 pl-4 text-xs leading-5 text-muted-foreground">
                {warnings.map((warning) => (
                  <li key={warning}>{warning}</li>
                ))}
              </ul>
            )}
          </div>
          <Button
            variant="ghost"
            size="sm"
            onClick={dismissCompatibilityNotice}
          >
            Dismiss
          </Button>
        </div>
      )}

      <section className="mt-8" aria-busy={results.isFetching}>
        {!query ? (
          <div className="rounded-xl border border-dashed bg-muted/10 px-6 py-14 text-center">
            <Dna className="mx-auto size-7 text-primary" />
            <h2 className="mt-4 font-medium">Enter a nucleotide sequence</h2>
            <p className="mx-auto mt-2 max-w-md text-sm leading-6 text-muted-foreground">
              Results remain scoped to designs visible to your account. Queries
              and matching modes are stored in the URL so the search can be
              shared and reproduced.
            </p>
          </div>
        ) : results.error ? (
          <div
            className="rounded-xl border border-destructive/25 bg-destructive/5 p-5"
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
          <div className="space-y-3" aria-label="Loading sequence matches">
            {Array.from({ length: 5 }).map((_, index) => (
              <Skeleton key={index} className="h-40 rounded-xl" />
            ))}
          </div>
        ) : results.data.items.length > 0 ? (
          <>
            <div
              className="mb-4 text-sm text-muted-foreground"
              aria-live="polite"
            >
              <span className="font-semibold tabular-nums text-foreground">
                {results.data.total.toLocaleString()}
              </span>{" "}
              {results.data.total === 1 ? "sequence match" : "sequence matches"}
            </div>
            <div className="space-y-3">
              {results.data.items.map((hit) => (
                <ObjectSummaryLink
                  key={hit.uri}
                  object={hit}
                  metadata={
                    <>
                      <span className="rounded-full bg-primary/10 px-2.5 py-0.5 text-xs font-semibold tabular-nums text-primary">
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
          </>
        ) : (
          <div className="rounded-xl border border-dashed bg-muted/10 px-6 py-14 text-center">
            <h2 className="font-medium">No sequence matches</h2>
            <p className="mx-auto mt-2 max-w-md text-sm leading-6 text-muted-foreground">
              Try exact mode for a known motif or a longer, more representative
              sequence in global mode.
            </p>
          </div>
        )}
      </section>
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
