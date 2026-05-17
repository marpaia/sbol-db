/**
 * Per-ontology browser. Reached by clicking an ontology card on the
 * `/ontologies` index. Shows:
 *
 *  - A header with the ontology prefix, name, version, and source URL
 *    plus a back link to the index.
 *  - A debounced search box (matches curie or name, case-insensitive).
 *  - A paginated term list. Each row is collapsible — click to reveal
 *    definition, synonyms, and a deep link into the lookup tool on the
 *    index page.
 *
 * Pagination is offset-based and capped at 100/page. The server returns
 * the total count alongside the page so we can render "Page X of Y"
 * without a second round-trip.
 */

import { useEffect, useMemo, useState } from "react";
import {
  ChevronLeft,
  ChevronRight,
  Library,
  Search,
  TriangleAlert,
} from "lucide-react";
import { Link, useNavigate, useParams } from "react-router-dom";

import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { useOntologies, useOntologyTerms } from "@/hooks/useOntologies";
import type { OntologyTermRecord } from "@/lib/api";

const PAGE_SIZE = 100;

export default function OntologyDetailRoute() {
  const params = useParams<{ prefix: string }>();
  const prefix = (params.prefix ?? "").toUpperCase();

  const { data: ontologies, isLoading: ontoLoading } = useOntologies();
  const ontology = useMemo(
    () => ontologies?.find((o) => o.prefix.toUpperCase() === prefix),
    [ontologies, prefix]
  );

  const [draft, setDraft] = useState("");
  const [search, setSearch] = useState("");
  const [page, setPage] = useState(0);

  // Debounce the search box: 250ms after the user stops typing, commit
  // the filter and reset to the first page.
  useEffect(() => {
    const t = setTimeout(() => {
      setSearch(draft.trim());
      setPage(0);
    }, 250);
    return () => clearTimeout(t);
  }, [draft]);

  const {
    data: termsPage,
    isLoading: termsLoading,
    error: termsError,
  } = useOntologyTerms({
    prefix,
    q: search || undefined,
    limit: PAGE_SIZE,
    offset: page * PAGE_SIZE,
  });

  const totalPages = termsPage
    ? Math.max(1, Math.ceil(termsPage.total / PAGE_SIZE))
    : 1;

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-6 px-8 py-10">
        <Link
          to="/ontologies"
          className="inline-flex items-center gap-1 text-xs text-muted-foreground transition-colors hover:text-foreground"
        >
          <ChevronLeft size={12} />
          All ontologies
        </Link>

        <header className="flex flex-wrap items-start justify-between gap-4">
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <Library
                size={16}
                className="shrink-0 text-muted-foreground/70"
              />
              <h1 className="font-mono text-xl font-semibold tracking-tight">
                {prefix.toLowerCase()}
              </h1>
              {ontology?.version && (
                <span className="rounded-sm bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
                  {ontology.version}
                </span>
              )}
            </div>
            {ontology ? (
              <>
                <div className="mt-1 text-sm text-foreground">
                  {ontology.name}
                </div>
                <div className="mt-1 text-xs tabular-nums text-muted-foreground">
                  {ontology.term_count.toLocaleString()} terms
                </div>
                {ontology.source_url && (
                  <div className="mt-2 truncate font-mono text-[11px] text-muted-foreground/70">
                    {ontology.source_url}
                  </div>
                )}
              </>
            ) : ontoLoading ? (
              <div className="mt-2 h-4 w-48 animate-pulse rounded bg-muted" />
            ) : (
              <div className="mt-2 text-sm text-muted-foreground">
                This prefix isn't loaded.
              </div>
            )}
          </div>
        </header>

        <div className="relative">
          <Search
            size={14}
            className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-muted-foreground"
          />
          <input
            type="text"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder="Filter by curie or name…"
            className="w-full rounded-md border bg-background py-2 pl-8 pr-3 text-sm text-foreground outline-none placeholder:text-muted-foreground/60 focus:ring-1 focus:ring-ring"
          />
        </div>

        {termsError ? (
          <ErrorBanner
            title="Couldn't list terms"
            body={(termsError as Error).message}
          />
        ) : termsLoading && !termsPage ? (
          <TermListSkeleton />
        ) : !termsPage || termsPage.terms.length === 0 ? (
          <div className="rounded-lg border bg-card px-4 py-6 text-sm text-muted-foreground">
            {search
              ? `No terms match "${search}".`
              : "This ontology has no terms."}
          </div>
        ) : (
          <>
            <PageStatus
              total={termsPage.total}
              page={page}
              pageSize={PAGE_SIZE}
              search={search}
            />
            <ul className="divide-y rounded-lg border bg-card">
              {termsPage.terms.map((t) => (
                <TermRow key={t.iri} term={t} />
              ))}
            </ul>
            <Pagination page={page} totalPages={totalPages} onPage={setPage} />
          </>
        )}
      </div>
    </div>
  );
}

function PageStatus({
  total,
  page,
  pageSize,
  search,
}: {
  total: number;
  page: number;
  pageSize: number;
  search: string;
}) {
  const start = total === 0 ? 0 : page * pageSize + 1;
  const end = Math.min((page + 1) * pageSize, total);
  return (
    <div className="text-xs text-muted-foreground">
      Showing{" "}
      <span className="tabular-nums text-foreground">
        {start.toLocaleString()}–{end.toLocaleString()}
      </span>{" "}
      of{" "}
      <span className="tabular-nums text-foreground">
        {total.toLocaleString()}
      </span>{" "}
      {search ? (
        <>
          matches for <code className="font-mono">{search}</code>
        </>
      ) : (
        "terms"
      )}
    </div>
  );
}

function TermRow({ term }: { term: OntologyTermRecord }) {
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  return (
    <li>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-3 px-4 py-2.5 text-left transition-colors hover:bg-accent/40"
      >
        <ChevronRight
          size={12}
          className={`shrink-0 transition-transform ${
            open ? "rotate-90 text-foreground" : "text-muted-foreground"
          }`}
        />
        <span className="font-mono text-xs font-medium text-foreground">
          {term.curie}
        </span>
        <span className="min-w-0 flex-1 truncate text-sm text-foreground">
          {term.name}
        </span>
        {term.is_obsolete && (
          <span className="inline-flex items-center gap-1 rounded-sm bg-destructive/10 px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-destructive">
            <TriangleAlert size={10} />
            obsolete
          </span>
        )}
      </button>
      {open && (
        <div className="space-y-3 border-t bg-background/40 px-4 py-3 pl-9">
          <div className="truncate font-mono text-[11px] text-muted-foreground/80">
            {term.iri}
          </div>
          {term.definition && (
            <div className="text-sm text-foreground">{term.definition}</div>
          )}
          {term.synonyms.length > 0 && (
            <div>
              <div className="mb-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
                Synonyms ({term.synonyms.length})
              </div>
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
          <button
            type="button"
            onClick={() =>
              navigate(`/ontologies?lookup=${encodeURIComponent(term.curie)}`)
            }
            className="text-xs font-medium text-foreground underline underline-offset-2 hover:text-foreground/80"
          >
            Open in term lookup →
          </button>
        </div>
      )}
    </li>
  );
}

function Pagination({
  page,
  totalPages,
  onPage,
}: {
  page: number;
  totalPages: number;
  onPage: (p: number) => void;
}) {
  if (totalPages <= 1) return null;
  return (
    <div className="flex items-center justify-between gap-2">
      <button
        type="button"
        onClick={() => onPage(Math.max(0, page - 1))}
        disabled={page === 0}
        className="inline-flex items-center gap-1 rounded-md border px-2.5 py-1.5 text-xs font-medium transition-colors hover:bg-accent/40 disabled:cursor-not-allowed disabled:opacity-50"
      >
        <ChevronLeft size={12} />
        Previous
      </button>
      <div className="text-xs tabular-nums text-muted-foreground">
        Page {page + 1} of {totalPages}
      </div>
      <button
        type="button"
        onClick={() => onPage(Math.min(totalPages - 1, page + 1))}
        disabled={page >= totalPages - 1}
        className="inline-flex items-center gap-1 rounded-md border px-2.5 py-1.5 text-xs font-medium transition-colors hover:bg-accent/40 disabled:cursor-not-allowed disabled:opacity-50"
      >
        Next
        <ChevronRight size={12} />
      </button>
    </div>
  );
}

function TermListSkeleton() {
  return (
    <div className="divide-y rounded-lg border bg-card">
      {Array.from({ length: 6 }).map((_, i) => (
        <div key={i} className="flex items-center gap-3 px-4 py-3">
          <div className="h-3 w-3 animate-pulse rounded bg-muted" />
          <div className="h-3 w-20 animate-pulse rounded bg-muted" />
          <div className="h-3 flex-1 animate-pulse rounded bg-muted" />
        </div>
      ))}
    </div>
  );
}
