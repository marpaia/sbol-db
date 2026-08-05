import { type FormEvent, useEffect, useState } from "react";
import { Dna, FileSearch, Network, Search, Tags } from "lucide-react";

import { HowSearchWorks } from "@/components/portal/HowSearchWorks";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { SearchMethod } from "@/features/registry/discovery/search-methods";
import { cn } from "@/lib/utils";

const SEQUENCE_EXAMPLES = ["GAATTC", "GGTACC", "AAGCTT"];

export function UnifiedSearchInput({
  methods,
  method,
  query,
  strategiesLoading,
  strategiesError,
  onMethodChange,
  onSearch,
}: {
  methods: SearchMethod[];
  method: SearchMethod;
  query: string;
  strategiesLoading: boolean;
  strategiesError?: string;
  onMethodChange: (method: SearchMethod) => void;
  onSearch: (query: string) => void;
}) {
  const [draft, setDraft] = useState(query);
  const sequenceInput = method.input === "sequence";
  const intents = buildSearchIntents(methods);
  const intent =
    intents.find((candidate) =>
      candidate.methods.some(
        (candidateMethod) => candidateMethod.key === method.key
      )
    ) ?? intents[0];
  const MethodIcon = intent.icon;

  useEffect(() => setDraft(query), [query]);

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const value = sequenceInput ? normalizeSequence(draft) : draft.trim();
    onSearch(value);
  };

  return (
    <form
      role="search"
      className="mt-6 w-full rounded-xl border bg-card p-4 shadow-sm sm:p-5"
      onSubmit={submit}
    >
      <fieldset>
        <legend className="text-xs font-medium text-muted-foreground">
          What do you want to search by?
        </legend>
        <div
          className={cn(
            "mt-2 grid gap-2",
            intents.length === 2
              ? "sm:grid-cols-2"
              : intents.length === 3
                ? "sm:grid-cols-3"
                : "sm:grid-cols-2 lg:grid-cols-4"
          )}
        >
          {intents.map((candidate) => {
            const active = candidate.key === intent.key;
            const IntentIcon = candidate.icon;
            return (
              <button
                key={candidate.key}
                type="button"
                aria-pressed={active}
                className={cn(
                  "flex min-w-0 items-start gap-3 rounded-lg border px-3 py-3 text-left transition-[border-color,background-color,color,transform] duration-150 [transition-timing-function:cubic-bezier(0.23,1,0.32,1)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 active:scale-[0.985] motion-reduce:transition-none",
                  active
                    ? "border-primary/40 bg-primary/10 text-foreground shadow-sm"
                    : "bg-background text-muted-foreground hover:border-primary/25 hover:text-foreground"
                )}
                onClick={() => onMethodChange(preferredMethod(candidate))}
              >
                <span
                  className={cn(
                    "flex size-8 shrink-0 items-center justify-center rounded-md",
                    active
                      ? "bg-primary/10 text-primary"
                      : "bg-muted text-muted-foreground"
                  )}
                >
                  <IntentIcon className="size-4" />
                </span>
                <span className="min-w-0">
                  <span className="block text-sm font-semibold leading-5">
                    {candidate.label}
                  </span>
                  <span className="mt-0.5 block text-[11px] leading-4 text-muted-foreground">
                    {candidate.hint}
                  </span>
                </span>
              </button>
            );
          })}
        </div>
      </fieldset>

      <div className="mt-4 grid w-full min-w-0 gap-2 sm:grid-cols-[minmax(0,1fr)_auto]">
        <div className="relative min-w-0">
          <MethodIcon
            aria-hidden="true"
            className="pointer-events-none absolute left-3 top-1/2 z-10 size-4 -translate-y-1/2 text-muted-foreground"
          />
          <Input
            id="search-query"
            value={draft}
            onChange={(event) =>
              setDraft(
                sequenceInput
                  ? event.target.value.toUpperCase()
                  : event.target.value
              )
            }
            placeholder={placeholderFor(method)}
            aria-label={labelFor(method)}
            autoFocus
            autoComplete="off"
            autoCapitalize={sequenceInput ? "characters" : undefined}
            spellCheck={!sequenceInput}
            className={cn(
              "h-11 w-full min-w-0 bg-background pl-10 shadow-sm",
              sequenceInput && "font-mono"
            )}
          />
        </div>

        <Button
          type="submit"
          className="h-11 w-full px-5 sm:w-auto"
          disabled={sequenceInput && !normalizeSequence(draft)}
        >
          <Search /> Search
        </Button>
      </div>

      <div className="mt-3 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs leading-5 text-muted-foreground">
        <span>{intent.description}</span>
      </div>

      <SearchIntentDetails
        intent={intent}
        method={method}
        onMethodChange={onMethodChange}
      />

      {sequenceInput && (
        <div className="mt-3 flex flex-wrap items-center gap-1.5 border-t pt-3 text-xs text-muted-foreground">
          <span className="mr-0.5">Try a motif</span>
          {SEQUENCE_EXAMPLES.map((example) => (
            <button
              key={example}
              type="button"
              className="rounded-md border bg-background px-2 py-1 font-mono text-[11px] text-foreground transition-[color,background-color,transform] duration-150 [transition-timing-function:cubic-bezier(0.23,1,0.32,1)] hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring active:scale-[0.97] motion-reduce:transition-none"
              onClick={() => {
                setDraft(example);
                onSearch(example);
              }}
            >
              {example}
            </button>
          ))}
        </div>
      )}

      {strategiesError && (
        <p className="mt-3 border-t pt-3 text-xs leading-5 text-amber-700 dark:text-amber-300">
          Additional search methods could not be loaded. Names, filters, and DNA
          sequence search remain available. {strategiesError}
        </p>
      )}

      {strategiesLoading && (
        <p className="mt-3 text-xs text-muted-foreground" role="status">
          Loading additional search methods…
        </p>
      )}
    </form>
  );
}

type SearchIntent = {
  key: "names" | "meaning" | "sequence" | "related";
  label: string;
  hint: string;
  description: string;
  icon: typeof Search;
  methods: SearchMethod[];
};

function buildSearchIntents(methods: SearchMethod[]): SearchIntent[] {
  const names = methods.filter((method) => method.kind === "native");
  const meaning = methods.filter(
    (method) => method.kind === "structured" && method.input === "text"
  );
  const sequence = methods.filter((method) => method.input === "sequence");
  const related = methods.filter(
    (method) => method.kind === "structured" && method.input === "similar"
  );

  return [
    {
      key: "names" as const,
      label: "Names & filters",
      hint: "Names, identifiers, types, and roles",
      description:
        "Find a known design or browse the registry using precise biological filters.",
      icon: FileSearch,
      methods: names,
    },
    {
      key: "sequence" as const,
      label: "DNA sequence",
      hint: "Motifs or complete nucleotide sequences",
      description:
        "Find designs whose DNA contains a motif or closely matches a nucleotide sequence.",
      icon: Dna,
      methods: sequence,
    },
    {
      key: "meaning" as const,
      label: "Biological meaning",
      hint: "Functions, concepts, and descriptions",
      description:
        "Describe what a design does or represents, even if the same words are not used in its record.",
      icon: Tags,
      methods: meaning,
    },
    {
      key: "related" as const,
      label: "Related designs",
      hint: "Start from an existing design URI",
      description:
        "Use a known design as the starting point and find other related registry objects.",
      icon: Network,
      methods: related,
    },
  ].filter((intent) => intent.methods.length > 0);
}

function preferredMethod(intent: SearchIntent): SearchMethod {
  if (intent.key === "sequence") {
    return (
      intent.methods.find((method) => method.kind === "sequence") ??
      intent.methods.find(
        (method) => method.kind === "structured" && method.isDefault
      ) ??
      intent.methods[0]
    );
  }
  return (
    intent.methods.find(
      (method) => method.kind === "structured" && method.isDefault
    ) ?? intent.methods[0]
  );
}

function SearchIntentDetails({
  intent,
  method,
  onMethodChange,
}: {
  intent: SearchIntent;
  method: SearchMethod;
  onMethodChange: (method: SearchMethod) => void;
}) {
  return (
    <HowSearchWorks>
      <p>{searchBehavior(method)}</p>
      {intent.methods.length > 1 && (
        <SearchMethodSelector
          methods={intent.methods}
          method={method}
          onMethodChange={onMethodChange}
        />
      )}
      <div className="mt-3 border-t pt-3">
        <SearchMethodTechnicalSummary method={method} />
      </div>
    </HowSearchWorks>
  );
}

function SearchMethodSelector({
  methods,
  method,
  onMethodChange,
}: {
  methods: SearchMethod[];
  method: SearchMethod;
  onMethodChange: (method: SearchMethod) => void;
}) {
  return (
    <fieldset className="mt-3 border-t pt-3">
      <legend className="font-medium text-foreground">Available methods</legend>
      <div className="mt-2 grid gap-2 sm:grid-cols-2">
        {methods.map((candidate) => {
          const active = candidate.key === method.key;
          return (
            <button
              key={candidate.key}
              type="button"
              aria-pressed={active}
              className={cn(
                "rounded-md border bg-background px-3 py-2 text-left transition-[border-color,background-color,transform] duration-150 [transition-timing-function:cubic-bezier(0.23,1,0.32,1)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring active:scale-[0.985] motion-reduce:transition-none",
                active && "border-primary/40 bg-primary/5"
              )}
              onClick={() => onMethodChange(candidate)}
            >
              <span className="flex items-center gap-2 font-medium text-foreground">
                {methodName(candidate)}
                {candidate.kind === "structured" && candidate.isDefault && (
                  <Badge variant="secondary" className="text-[9px]">
                    Recommended
                  </Badge>
                )}
              </span>
              <span className="mt-1 block leading-5">
                {candidate.description}
              </span>
            </button>
          );
        })}
      </div>
    </fieldset>
  );
}

function SearchMethodTechnicalSummary({ method }: { method: SearchMethod }) {
  if (method.kind === "native") {
    return (
      <>
        <p className="font-medium text-foreground">Registry record search</p>
        <SearchCapabilityBadges
          labels={["offset paging", "exact totals", "database filters"]}
        />
      </>
    );
  }

  if (method.kind === "sequence") {
    return (
      <>
        <p className="font-medium text-foreground">
          Built-in nucleotide search
        </p>
        <SearchCapabilityBadges
          labels={["global + exact", "both DNA strands", "exact totals"]}
        />
      </>
    );
  }

  const { capabilities, requirements } = method.strategy;
  return (
    <>
      <p className="font-medium text-foreground">
        {method.strategy.display_name} · version {method.strategy.version}
      </p>
      <p className="mt-1">{method.strategy.description}</p>
      <SearchCapabilityBadges
        labels={[
          `${method.input} input`,
          `${capabilities.pagination.replaceAll("_", " ")} paging`,
          `${capabilities.totals.replaceAll("_", " ")} totals`,
          `${capabilities.filter_execution.replaceAll("_", " ")} filters`,
        ]}
      />
      <SearchRequirements
        label="Embedding profiles"
        values={requirements.embedding_profiles}
      />
      <SearchRequirements
        label="Vector indexes"
        values={requirements.vector_indexes}
      />
      <SearchRequirements
        label="Candidate sources"
        values={requirements.candidate_sources}
      />
    </>
  );
}

function SearchCapabilityBadges({ labels }: { labels: string[] }) {
  return (
    <div className="mt-2 flex flex-wrap gap-1.5">
      {labels.map((label) => (
        <Badge key={label} variant="outline" className="text-[10px]">
          {label}
        </Badge>
      ))}
    </div>
  );
}

function SearchRequirements({
  label,
  values,
}: {
  label: string;
  values?: string[];
}) {
  if (!values?.length) return null;
  return (
    <div className="mt-3">
      <p className="font-medium text-foreground">{label}</p>
      <p className="mt-0.5 break-words font-mono text-[10px]">
        {values.join(", ")}
      </p>
    </div>
  );
}

function searchBehavior(method: SearchMethod): string {
  if (method.kind === "native") {
    return "Looks for exact words and identifiers in registry records, then applies object type, biological role, ownership, provenance, and date filters. Result counts are exact for the objects you can access.";
  }
  if (method.kind === "sequence") {
    return "Compares the query with visible DNA sequences on both strands. Global alignment ranks similar sequences; exact mode finds the query as a contiguous motif.";
  }
  if (method.input === "similar") {
    return "Uses the supplied design URI as a reference and ranks other visible registry objects by biological relatedness.";
  }
  if (method.input === "sequence") {
    return "Compares the nucleotide query with a specialized sequence model configured for this registry. Available matching controls depend on that method's capabilities.";
  }
  return "Compares the biological context of your description with names, descriptions, types, roles, and other canonical SBOL metadata. Results are ranked by conceptual similarity rather than exact wording alone.";
}

function methodName(method: SearchMethod): string {
  if (method.kind === "native") return "Registry records";
  if (method.kind === "sequence") return "Exact and alignment search";
  return method.strategy.display_name;
}

function placeholderFor(method: SearchMethod): string {
  if (method.input === "sequence") {
    return "Enter DNA using IUPAC nucleotide codes…";
  }
  if (method.input === "similar") {
    return "Enter the URI of a design to find related objects…";
  }
  if (method.kind === "structured") {
    return "Describe a biological function, part, or design…";
  }
  return "Search by name, description, identifier, or biological concept…";
}

function labelFor(method: SearchMethod): string {
  if (method.input === "sequence") return "DNA sequence query";
  if (method.input === "similar") return "Reference design URI";
  if (method.kind === "structured") return "Biological meaning query";
  return "Name, identifier, or keyword query";
}

function normalizeSequence(value: string): string {
  return value.replace(/\s+/g, "").toUpperCase();
}
