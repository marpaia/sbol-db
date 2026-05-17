/**
 * Ontology browser. Three surfaces:
 *
 *  - Loaded ontologies: every prefix the server has imported, with
 *    term counts, source URL, and version. "Load…" opens the same
 *    loader dialog used on the dashboard.
 *  - Term lookup: enter a CURIE (`SO:0000167`) or full IRI; the
 *    server resolves it via `/ontology/term` and the result panel
 *    shows the name, definition, synonyms, and obsolete flag.
 *  - Descendants: optional expansion of the transitive `is_a`
 *    closure beneath the resolved term, sorted by depth.
 *
 * The server has no list-all-terms endpoint, so the browser is
 * lookup-driven rather than paginated. A descendants view gives the
 * user a way to fan out from any known term into its sub-hierarchy.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Link, useSearchParams } from "react-router-dom";
import {
  ChevronRight,
  Library,
  Loader2,
  Plus,
  Search,
  TriangleAlert,
} from "lucide-react";

import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { OntologyLoaderDialog } from "@/components/lab/OntologyLoaderDialog";
import {
  useOntologies,
  useOntologyDescendants,
  useOntologyTerm,
} from "@/hooks/useOntologies";
import type { OntologyRecord, OntologyTermRecord } from "@/lib/api";
import { ApiError } from "@/lib/api";

export default function OntologyRoute() {
  const queryClient = useQueryClient();
  const { data: ontologies, isLoading, error } = useOntologies();
  const [loaderOpen, setLoaderOpen] = useState(false);

  const onLoaded = useCallback(() => {
    queryClient.invalidateQueries({ queryKey: ["lab", "ontologies"] });
    queryClient.invalidateQueries({ queryKey: ["lab", "overview"] });
    queryClient.invalidateQueries({ queryKey: ["lab", "schema", "sparql"] });
  }, [queryClient]);

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-8 px-8 py-10">
        <header className="flex flex-wrap items-start justify-between gap-4">
          <div>
            <h1 className="text-2xl font-semibold tracking-tight">
              Ontologies
            </h1>
            <p className="mt-2 text-sm text-muted-foreground">
              Browse the OBO ontologies loaded into this database, look up
              terms by CURIE or IRI, and explore their <code>is_a</code>{" "}
              descendants.
            </p>
          </div>
          <button
            type="button"
            onClick={() => setLoaderOpen(true)}
            className="inline-flex items-center gap-1.5 rounded-md bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90"
          >
            <Plus size={14} />
            Load ontology
          </button>
        </header>

        <section>
          <SectionLabel>Loaded</SectionLabel>
          {error ? (
            <ErrorBanner
              title="Couldn't list ontologies"
              body={(error as Error).message}
            />
          ) : isLoading || !ontologies ? (
            <OntologyListSkeleton />
          ) : ontologies.length === 0 ? (
            <Empty>
              No ontologies loaded yet.{" "}
              <button
                type="button"
                onClick={() => setLoaderOpen(true)}
                className="font-medium text-foreground hover:underline"
              >
                Load one
              </button>{" "}
              to get started.
            </Empty>
          ) : (
            <div className="grid gap-3 sm:grid-cols-2">
              {ontologies.map((o) => (
                <OntologyCard key={o.prefix} ontology={o} />
              ))}
            </div>
          )}
        </section>

        <section>
          <SectionLabel>Term lookup</SectionLabel>
          <TermLookup />
        </section>
      </div>

      <OntologyLoaderDialog
        open={loaderOpen}
        onOpenChange={setLoaderOpen}
        onLoaded={onLoaded}
        loadedPrefixes={ontologies?.map((o) => o.prefix) ?? []}
      />
    </div>
  );
}

function OntologyCard({ ontology }: { ontology: OntologyRecord }) {
  return (
    <Link
      to={`/ontologies/${ontology.prefix.toLowerCase()}`}
      className="group block rounded-lg border bg-card p-4 transition-colors hover:bg-accent/40 hover:border-foreground/20"
    >
      <header className="flex items-center gap-2">
        <Library size={14} className="shrink-0 text-muted-foreground/70" />
        <span className="font-mono text-sm font-medium text-foreground">
          {ontology.prefix.toLowerCase()}
        </span>
        {ontology.version && (
          <span className="rounded-sm bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
            {ontology.version}
          </span>
        )}
        <span className="ml-auto text-xs tabular-nums text-muted-foreground">
          {ontology.term_count.toLocaleString()} terms
        </span>
        <ChevronRight
          size={14}
          className="text-muted-foreground/60 transition-transform group-hover:translate-x-0.5 group-hover:text-foreground"
        />
      </header>
      <div className="mt-1.5 text-sm text-foreground">{ontology.name}</div>
      {ontology.source_url && (
        <div className="mt-2 truncate font-mono text-[11px] text-muted-foreground/70">
          {ontology.source_url}
        </div>
      )}
      <div className="mt-2 text-[11px] text-muted-foreground/70">
        imported {formatRelative(ontology.imported_at)}
      </div>
    </Link>
  );
}

function TermLookup() {
  const [searchParams, setSearchParams] = useSearchParams();
  const lookupParam = searchParams.get("lookup") ?? "";
  const [draft, setDraft] = useState(lookupParam);
  const [submitted, setSubmitted] = useState(lookupParam);

  // Deep links (`/ontologies?lookup=SBO:0000515`) from per-ontology
  // browsers should auto-trigger the lookup. Sync both fields when the
  // URL changes, and clear the param after we've consumed it so the
  // back button doesn't keep re-triggering.
  useEffect(() => {
    if (!lookupParam) return;
    setDraft(lookupParam);
    setSubmitted(lookupParam);
    setSearchParams({}, { replace: true });
  }, [lookupParam, setSearchParams]);

  const { data, isLoading, error, isFetching } = useOntologyTerm(submitted);

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    setSubmitted(draft.trim());
  };

  return (
    <div className="space-y-4">
      <form onSubmit={submit} className="flex items-center gap-2">
        <div className="relative flex-1">
          <Search
            size={14}
            className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-muted-foreground"
          />
          <input
            type="text"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder="SO:0000167  or  http://purl.obolibrary.org/obo/SO_0000167"
            className="w-full rounded-md border bg-background py-2 pl-8 pr-3 text-sm text-foreground outline-none placeholder:text-muted-foreground/60 focus:ring-1 focus:ring-ring"
          />
        </div>
        <button
          type="submit"
          disabled={!draft.trim() || isFetching}
          className="rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:bg-muted disabled:text-muted-foreground"
        >
          Look up
        </button>
      </form>

      {submitted ? (
        isLoading ? (
          <Pending label={`Looking up ${submitted}…`} />
        ) : error ? (
          <LookupError iri={submitted} error={error} />
        ) : data ? (
          <TermResult term={data} />
        ) : null
      ) : (
        <p className="text-xs text-muted-foreground">
          Enter a CURIE or full IRI. The lookup hits{" "}
          <code className="font-mono">/ontology/term</code>; descendants are
          fetched on demand.
        </p>
      )}
    </div>
  );
}

function TermResult({ term }: { term: OntologyTermRecord }) {
  return (
    <div className="space-y-4">
      <article className="rounded-lg border bg-card">
        <header className="flex flex-wrap items-center gap-2 border-b px-4 py-3">
          <span className="font-mono text-sm font-medium text-foreground">
            {term.curie}
          </span>
          {term.is_obsolete && (
            <span className="inline-flex items-center gap-1 rounded-sm bg-destructive/10 px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-destructive">
              <TriangleAlert size={10} />
              obsolete
            </span>
          )}
          <span className="ml-auto truncate font-mono text-[11px] text-muted-foreground/70">
            {term.iri}
          </span>
        </header>
        <div className="space-y-3 px-4 py-4">
          <div>
            <FieldLabel>Name</FieldLabel>
            <div className="text-sm text-foreground">{term.name}</div>
          </div>
          {term.definition && (
            <div>
              <FieldLabel>Definition</FieldLabel>
              <div className="text-sm text-foreground">{term.definition}</div>
            </div>
          )}
          {term.synonyms.length > 0 && (
            <div>
              <FieldLabel>Synonyms ({term.synonyms.length})</FieldLabel>
              <div className="flex flex-wrap gap-1.5">
                {term.synonyms.map((s) => (
                  <span
                    key={s}
                    className="rounded-sm bg-muted px-1.5 py-0.5 text-xs text-foreground"
                  >
                    {s}
                  </span>
                ))}
              </div>
            </div>
          )}
        </div>
      </article>

      <DescendantsExplorer iri={term.iri} curie={term.curie} />
    </div>
  );
}

function DescendantsExplorer({ iri, curie }: { iri: string; curie: string }) {
  const [open, setOpen] = useState(false);
  // Reset the toggle when the user switches to a different term so we
  // don't kick off an expensive fetch they didn't ask for.
  useEffect(() => {
    setOpen(false);
  }, [iri]);

  const { data, isLoading, error, isFetching } = useOntologyDescendants(
    iri,
    open
  );

  const byDepth = useMemo(() => {
    if (!data) return [];
    const groups = new Map<number, string[]>();
    for (const d of data) {
      const arr = groups.get(d.depth) ?? [];
      arr.push(d.iri);
      groups.set(d.depth, arr);
    }
    return [...groups.entries()].sort((a, b) => a[0] - b[0]);
  }, [data]);

  return (
    <section className="rounded-lg border bg-card">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-2 border-b px-4 py-2.5 text-left transition-colors hover:bg-accent/40"
      >
        <ChevronRight
          size={14}
          className={`shrink-0 transition-transform ${
            open ? "rotate-90 text-foreground" : "text-muted-foreground"
          }`}
        />
        <h3 className="text-sm font-medium">Descendants</h3>
        <span className="text-xs text-muted-foreground">
          transitive <code>is_a</code> closure under {curie}
        </span>
        {open && data && (
          <span className="ml-auto text-xs tabular-nums text-muted-foreground">
            {data.length.toLocaleString()} terms
          </span>
        )}
      </button>
      {open && (
        <div className="px-4 py-3">
          {isLoading || isFetching ? (
            <Pending label="Computing closure…" />
          ) : error ? (
            <ErrorBanner
              title="Closure lookup failed"
              body={(error as Error).message}
            />
          ) : !data || data.length === 0 ? (
            <Empty>No descendants — this term is a leaf.</Empty>
          ) : (
            <ul className="space-y-3">
              {byDepth.map(([depth, iris]) => (
                <li key={depth}>
                  <div className="mb-1.5 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
                    Depth {depth} · {iris.length}{" "}
                    {iris.length === 1 ? "term" : "terms"}
                  </div>
                  <div className="flex flex-wrap gap-1.5">
                    {iris.map((d) => (
                      <span
                        key={d}
                        title={d}
                        className="rounded-sm border bg-background px-1.5 py-0.5 font-mono text-[11px] text-foreground"
                      >
                        {shortIri(d)}
                      </span>
                    ))}
                  </div>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </section>
  );
}

function LookupError({ iri, error }: { iri: string; error: unknown }) {
  const notFound = error instanceof ApiError && error.status === 404;
  if (notFound) {
    return (
      <div className="flex items-start gap-3 rounded-md border bg-muted/40 px-3 py-3 text-sm">
        <Search
          size={14}
          className="mt-0.5 shrink-0 text-muted-foreground"
          aria-hidden
        />
        <div>
          <div className="font-medium text-foreground">No match</div>
          <div className="mt-0.5 text-muted-foreground">
            No term found for <code className="font-mono">{iri}</code>. Make
            sure the ontology is loaded and the identifier is correct.
          </div>
        </div>
      </div>
    );
  }
  return (
    <ErrorBanner title="Lookup failed" body={(error as Error).message} />
  );
}

function Pending({ label }: { label: string }) {
  return (
    <div className="flex items-center gap-2 rounded-md border bg-muted/40 px-3 py-2 text-sm text-foreground">
      <Loader2 size={14} className="animate-spin" />
      <span>{label}</span>
    </div>
  );
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <h2 className="mb-3 text-[11px] font-medium uppercase tracking-wider text-muted-foreground">
      {children}
    </h2>
  );
}

function FieldLabel({ children }: { children: React.ReactNode }) {
  return (
    <div className="mb-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
      {children}
    </div>
  );
}

function OntologyListSkeleton() {
  return (
    <div className="grid gap-3 sm:grid-cols-2">
      {Array.from({ length: 2 }).map((_, i) => (
        <div key={i} className="h-28 animate-pulse rounded-lg border bg-card" />
      ))}
    </div>
  );
}

function Empty({ children }: { children: React.ReactNode }) {
  return <div className="py-3 text-sm text-muted-foreground">{children}</div>;
}

function shortIri(iri: string): string {
  const m = iri.match(/[#/]([^#/]+)$/);
  return m ? m[1] : iri;
}

function formatRelative(iso: string): string {
  const then = new Date(iso).getTime();
  const now = Date.now();
  const seconds = Math.floor((now - then) / 1000);
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}

