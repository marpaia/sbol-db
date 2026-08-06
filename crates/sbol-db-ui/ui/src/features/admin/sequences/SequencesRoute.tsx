/**
 * Nucleotide motif search workbench. Two modes:
 *
 *  - Single: one motif, hit list. Forward + reverse-complement by
 *    default; toggle off for forward-only. Hits link into the object
 *    detail page for the matched `sbol:Sequence`.
 *  - Batch: up to 256 motifs (one per line). The server processes them
 *    in one call and returns one match group per pattern, preserving
 *    input order.
 *
 * Patterns shorter than 8 bp fall off the k-mer seed index onto a full
 * candidate scan, which can be much slower. The form surfaces
 * a hint when that happens so the user knows what they're paying for.
 */

import { useMemo, useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { Download, Loader2, Search, TriangleAlert } from "lucide-react";

import { AdminPage } from "@/components/admin/AdminPage";
import { DataTable, type DataTableColumn } from "@/components/lab/DataTable";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import {
  fetchCatalogSequences,
  sequenceSearch,
  sequenceSearchBatch,
  type BatchSequenceMatch,
  type CatalogSequence,
  type SequenceMatch,
} from "@/features/admin/sequences/api";
import { useWorkbenchStore } from "@/features/admin/workbench/store";
import { adminPath } from "@/lib/routes";
import { describeError } from "@/lib/utils";

const KMER_SEED_BP = 8;
const SINGLE_MAX_HITS_DEFAULT = 1024;
const BATCH_MAX_PATTERNS = 256;
const CATALOG_PAGE_SIZE = 100;

type Mode = "single" | "batch";

export default function SequencesRoute() {
  const navigate = useNavigate();
  const recent = useWorkbenchStore((s) => s.recentSeqPatterns);
  const remember = useWorkbenchStore((s) => s.rememberSeqPattern);

  const [mode, setMode] = useState<Mode>("single");
  const [pattern, setPattern] = useState("");
  const [batchText, setBatchText] = useState("");
  const [maxHits, setMaxHits] = useState<number>(SINGLE_MAX_HITS_DEFAULT);
  const [forwardOnly, setForwardOnly] = useState(false);

  const single = useMutation<SequenceMatch[], Error, void>({
    mutationFn: async () => {
      remember(pattern);
      return sequenceSearch({
        pattern: pattern.trim(),
        max_hits: maxHits,
        forward_only: forwardOnly,
      });
    },
  });

  const batchPatterns = useMemo(
    () =>
      batchText
        .split(/\r?\n/)
        .map((s) => s.trim())
        .filter((s) => s.length > 0),
    [batchText]
  );

  const batch = useMutation<BatchSequenceMatch[], Error, void>({
    mutationFn: () =>
      sequenceSearchBatch({
        patterns: batchPatterns,
        max_hits: maxHits,
        forward_only: forwardOnly,
      }),
  });

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    if (mode === "single") {
      if (!pattern.trim()) return;
      single.mutate();
    } else {
      if (batchPatterns.length === 0) return;
      batch.mutate();
    }
  };

  const shortSingle =
    pattern.trim().length > 0 && pattern.trim().length < KMER_SEED_BP;
  const tooManyBatch = batchPatterns.length > BATCH_MAX_PATTERNS;
  const shortInBatch = batchPatterns.some((p) => p.length < KMER_SEED_BP);

  const onOpenObject = (iri: string) =>
    navigate(adminPath(`/objects/${encodeURIComponent(iri)}`));
  return (
    <AdminPage
      title="Sequence index"
      description={`Browse every RDF Sequence resource, then run exact-match nucleotide searches with a ${KMER_SEED_BP}-base k-mer seed when sequence content is available.`}
      eyebrow="Data model"
    >
      <SequenceCatalog onOpenObject={onOpenObject} />

      <div className="flex items-center gap-1 border-b">
        <ModeTab active={mode === "single"} onClick={() => setMode("single")}>
          Single
        </ModeTab>
        <ModeTab active={mode === "batch"} onClick={() => setMode("batch")}>
          Batch
        </ModeTab>
      </div>

      <form onSubmit={submit} className="space-y-3">
        {mode === "single" ? (
          <div className="rounded-lg border bg-card px-4 py-3">
            <label className="block">
              <span className="mb-1 block text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
                Motif
              </span>
              <input
                type="text"
                value={pattern}
                onChange={(e) => setPattern(e.target.value.toUpperCase())}
                placeholder="GAATTC"
                spellCheck={false}
                className="w-full rounded-md border bg-background px-3 py-2 font-mono text-sm text-foreground outline-none placeholder:text-muted-foreground/60 focus:ring-1 focus:ring-ring"
              />
            </label>
            {recent.length > 0 && (
              <div className="mt-2 flex flex-wrap items-center gap-1.5">
                <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
                  Recent
                </span>
                {recent.slice(0, 8).map((p) => (
                  <button
                    key={p}
                    type="button"
                    onClick={() => setPattern(p)}
                    className="rounded-sm border bg-background px-1.5 py-0.5 font-mono text-[10px] text-foreground transition-colors hover:bg-accent/40"
                  >
                    {p}
                  </button>
                ))}
              </div>
            )}
          </div>
        ) : (
          <div className="space-y-2">
            <textarea
              value={batchText}
              onChange={(e) => setBatchText(e.target.value.toUpperCase())}
              placeholder={"GAATTC\nGGTACC\nAAGCTT"}
              rows={10}
              spellCheck={false}
              className="block w-full resize-y rounded-md border bg-background px-3 py-2 font-mono text-xs text-foreground outline-none placeholder:text-muted-foreground/60 focus:ring-1 focus:ring-ring"
            />
            <div className="text-xs text-muted-foreground">
              <span
                className={`tabular-nums ${
                  tooManyBatch ? "text-destructive" : "text-foreground"
                }`}
              >
                {batchPatterns.length.toLocaleString()}
              </span>{" "}
              of {BATCH_MAX_PATTERNS.toLocaleString()} patterns
              {tooManyBatch && (
                <span className="ml-2 text-destructive">
                  Trim the list to submit.
                </span>
              )}
            </div>
          </div>
        )}

        <div className="flex flex-wrap items-center gap-3 rounded-lg border bg-card px-4 py-3">
          <label className="flex items-center gap-1.5 text-xs">
            <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
              Max hits
            </span>
            <input
              type="number"
              min={1}
              value={maxHits}
              onChange={(e) =>
                setMaxHits(Math.max(1, parseInt(e.target.value, 10) || 1))
              }
              className="w-24 rounded-md border bg-background px-2 py-1 text-xs tabular-nums text-foreground outline-none focus:ring-1 focus:ring-ring"
            />
          </label>
          <label className="flex items-center gap-1.5 text-xs text-foreground">
            <input
              type="checkbox"
              checked={forwardOnly}
              onChange={(e) => setForwardOnly(e.target.checked)}
            />
            Forward strand only
          </label>
          <button
            type="submit"
            disabled={
              mode === "single"
                ? !pattern.trim() || single.isPending
                : batchPatterns.length === 0 || tooManyBatch || batch.isPending
            }
            className="ml-auto inline-flex items-center gap-1.5 rounded-md bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:bg-muted disabled:text-muted-foreground"
          >
            {(mode === "single" ? single.isPending : batch.isPending) ? (
              <Loader2 size={14} className="animate-spin" />
            ) : (
              <Search size={14} />
            )}
            Search
          </button>
        </div>

        {(mode === "single" ? shortSingle : shortInBatch) && (
          <div className="flex items-start gap-2 rounded-md border border-amber-500/40 bg-amber-500/5 px-3 py-2 text-xs">
            <TriangleAlert
              size={12}
              className="mt-0.5 shrink-0 text-amber-500"
            />
            <span className="text-foreground">
              Patterns shorter than {KMER_SEED_BP} bp can't use the k-mer seed
              index and must scan the nucleotide candidate set. Expect slower
              results on large corpora.
            </span>
          </div>
        )}
      </form>

      {mode === "single" ? (
        <SingleResults
          loading={single.isPending}
          error={single.error}
          data={single.data ?? null}
          onOpenObject={onOpenObject}
        />
      ) : (
        <BatchResults
          loading={batch.isPending}
          error={batch.error}
          data={batch.data ?? null}
          onOpenObject={onOpenObject}
        />
      )}
    </AdminPage>
  );
}

function SequenceCatalog({
  onOpenObject,
}: {
  onOpenObject: (iri: string) => void;
}) {
  const [cursors, setCursors] = useState<string[]>([""]);
  const [searchDraft, setSearchDraft] = useState("");
  const [search, setSearch] = useState("");
  const after = cursors.at(-1) || undefined;
  const page = cursors.length - 1;
  const { data, error, isLoading } = useQuery({
    queryKey: ["admin", "sequences", search, after ?? "", CATALOG_PAGE_SIZE],
    queryFn: ({ signal }) =>
      fetchCatalogSequences(
        { after, limit: CATALOG_PAGE_SIZE, q: search || undefined },
        signal
      ),
    staleTime: 30_000,
    placeholderData: (previous) => previous,
  });
  const records = data?.items ?? [];
  const columns: DataTableColumn<CatalogSequence>[] = [
    {
      id: "sequence",
      header: "Sequence resource",
      width: 480,
      cell: (record) => (
        <div className="min-w-0">
          <div className="truncate font-mono text-[11px] text-muted-foreground">
            {record.iri}
          </div>
        </div>
      ),
      sortValue: (record) => record.iri,
      filterValue: (record) => record.iri,
    },
    {
      id: "length",
      header: "Length",
      width: 100,
      align: "right",
      cell: (record) =>
        record.elements?.length.toLocaleString() ?? (
          <span className="text-muted-foreground/60">—</span>
        ),
      sortValue: (record) => record.elements?.length ?? -1,
    },
    {
      id: "encoding",
      header: "Encoding",
      width: 220,
      cell: (record) => (
        <span className="font-mono text-[11px] text-muted-foreground">
          {shortIri(record.encoding_iri)}
        </span>
      ),
      sortValue: (record) => record.encoding_iri ?? "",
      filterValue: (record) => record.encoding_iri ?? "",
    },
    {
      id: "graphs",
      header: "Graphs",
      width: 80,
      align: "right",
      cell: (record) => record.graph_count.toLocaleString(),
      sortValue: (record) => record.graph_count,
    },
  ];

  return (
    <section className="space-y-3">
      <div>
        <div className="text-xs font-medium text-foreground">
          Sequence resources
        </div>
        <div className="mt-0.5 text-xs text-muted-foreground">
          Canonical SBOL 2 and SBOL 3 Sequence resources, independent of the
          storage backend.
        </div>
      </div>
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
          <input
            value={searchDraft}
            onChange={(event) => setSearchDraft(event.target.value)}
            className="w-full rounded-md border bg-background py-2 pl-9 pr-3 text-sm outline-none placeholder:text-muted-foreground focus:ring-1 focus:ring-ring"
            placeholder="Search sequence IRI or metadata"
            aria-label="Search sequences"
          />
        </div>
        <button
          type="submit"
          className="rounded-md border bg-background px-3 py-2 text-sm font-medium transition-colors hover:bg-accent"
        >
          Search
        </button>
        {(search || searchDraft) && (
          <button
            type="button"
            onClick={() => {
              setSearchDraft("");
              setSearch("");
              setCursors([""]);
            }}
            className="rounded-md px-3 py-2 text-sm font-medium text-muted-foreground transition-colors hover:bg-accent"
          >
            Clear
          </button>
        )}
      </form>
      {error ? (
        <ErrorBanner
          title="Couldn't list sequences"
          body={(error as Error).message}
        />
      ) : isLoading && !data ? (
        <div className="h-32 animate-pulse rounded-lg border bg-muted/40" />
      ) : records.length === 0 ? (
        <div className="rounded-lg border bg-card px-4 py-6 text-sm text-muted-foreground">
          {search
            ? "No sequence resources match that search."
            : "No sequence resources are present in the RDF catalog."}
        </div>
      ) : (
        <div className="overflow-hidden rounded-lg border bg-card">
          <DataTable
            columns={columns}
            rows={records}
            rowKey={(record) => record.iri}
            onRowClick={(record) => onOpenObject(record.iri)}
          />
        </div>
      )}
      {records.length > 0 && (
        <div className="flex items-center justify-between gap-2 text-xs">
          <span className="text-muted-foreground">Page {page + 1}</span>
          <div className="flex items-center gap-2">
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
                data?.next_cursor &&
                setCursors((values) => [...values, data.next_cursor!])
              }
              disabled={!data?.next_cursor}
              className="rounded-md border px-2.5 py-1 font-medium transition-colors hover:bg-accent/40 disabled:cursor-not-allowed disabled:opacity-50"
            >
              Next
            </button>
          </div>
        </div>
      )}
    </section>
  );
}

function SingleResults({
  loading,
  error,
  data,
  onOpenObject,
}: {
  loading: boolean;
  error: unknown;
  data: SequenceMatch[] | null;
  onOpenObject: (iri: string) => void;
}) {
  if (loading && !data) {
    return (
      <div className="flex items-center gap-2 rounded-lg border bg-card px-4 py-3 text-sm text-muted-foreground">
        <Loader2 size={14} className="animate-spin" />
        Searching…
      </div>
    );
  }
  if (error) {
    return <ErrorBanner title="Search failed" body={describeError(error)} />;
  }
  if (!data) return null;
  if (data.length === 0) {
    return (
      <div className="rounded-lg border bg-card px-4 py-6 text-sm text-muted-foreground">
        No hits.
      </div>
    );
  }

  return (
    <section>
      <div className="mb-2 flex items-center justify-between text-xs">
        <div className="text-muted-foreground">
          <span className="tabular-nums text-foreground">
            {data.length.toLocaleString()}
          </span>{" "}
          hits
        </div>
        <button
          type="button"
          onClick={() => downloadJson(data, "sequence-hits.json")}
          className="inline-flex items-center gap-1.5 rounded-md border bg-background px-2.5 py-1 font-medium transition-colors hover:bg-accent/40"
        >
          <Download size={12} />
          JSON
        </button>
      </div>
      <div className="overflow-hidden rounded-lg border bg-card">
        <DataTable
          columns={matchColumns()}
          rows={data}
          rowKey={(m) => `${m.sequence_iri}|${m.start}|${m.strand}`}
          filterable
          onRowClick={(m) => onOpenObject(m.sequence_iri)}
        />
      </div>
    </section>
  );
}

function BatchResults({
  loading,
  error,
  data,
  onOpenObject,
}: {
  loading: boolean;
  error: unknown;
  data: BatchSequenceMatch[] | null;
  onOpenObject: (iri: string) => void;
}) {
  if (loading && !data) {
    return (
      <div className="flex items-center gap-2 rounded-lg border bg-card px-4 py-3 text-sm text-muted-foreground">
        <Loader2 size={14} className="animate-spin" />
        Searching…
      </div>
    );
  }
  if (error) {
    return (
      <ErrorBanner title="Batch search failed" body={describeError(error)} />
    );
  }
  if (!data) return null;
  if (data.length === 0) {
    return (
      <div className="rounded-lg border bg-card px-4 py-6 text-sm text-muted-foreground">
        No patterns to run.
      </div>
    );
  }

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between text-xs">
        <div className="text-muted-foreground">
          <span className="tabular-nums text-foreground">
            {data.length.toLocaleString()}
          </span>{" "}
          patterns, total{" "}
          <span className="tabular-nums text-foreground">
            {data
              .reduce((acc, g) => acc + g.matches.length, 0)
              .toLocaleString()}
          </span>{" "}
          hits
        </div>
        <button
          type="button"
          onClick={() => downloadJson(data, "sequence-batch.json")}
          className="inline-flex items-center gap-1.5 rounded-md border bg-background px-2.5 py-1 font-medium transition-colors hover:bg-accent/40"
        >
          <Download size={12} />
          JSON
        </button>
      </div>
      {data.map((group, i) => (
        <BatchGroup
          key={`${group.pattern}-${i}`}
          group={group}
          onOpenObject={onOpenObject}
        />
      ))}
    </div>
  );
}

function BatchGroup({
  group,
  onOpenObject,
}: {
  group: BatchSequenceMatch;
  onOpenObject: (iri: string) => void;
}) {
  const [open, setOpen] = useState(group.matches.length > 0);
  return (
    <section className="rounded-lg border bg-card">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-2 border-b px-4 py-2.5 text-left text-sm transition-colors hover:bg-accent/40"
      >
        <span className="font-mono text-foreground">{group.pattern}</span>
        <span className="ml-auto text-xs tabular-nums text-muted-foreground">
          {group.matches.length.toLocaleString()}{" "}
          {group.matches.length === 1 ? "hit" : "hits"}
        </span>
      </button>
      {open && (
        <div className="px-1 py-1">
          {group.matches.length === 0 ? (
            <div className="px-3 py-3 text-xs text-muted-foreground">
              No hits.
            </div>
          ) : (
            <DataTable
              columns={matchColumns()}
              rows={group.matches}
              rowKey={(m) => `${m.sequence_iri}|${m.start}|${m.strand}`}
              filterable
              onRowClick={(m) => onOpenObject(m.sequence_iri)}
            />
          )}
        </div>
      )}
    </section>
  );
}

function matchColumns(): DataTableColumn<SequenceMatch>[] {
  return [
    {
      id: "sequence",
      header: "Sequence IRI",
      width: 460,
      cell: (m) => (
        <span className="truncate font-mono text-[11px] text-foreground">
          {m.sequence_iri}
        </span>
      ),
      sortValue: (m) => m.sequence_iri,
      filterValue: (m) => m.sequence_iri,
    },
    {
      id: "start",
      header: "Start",
      width: 90,
      align: "right",
      cell: (m) => m.start.toLocaleString(),
      sortValue: (m) => m.start,
    },
    {
      id: "length",
      header: "Length",
      width: 80,
      align: "right",
      cell: (m) => m.length.toLocaleString(),
      sortValue: (m) => m.length,
    },
    {
      id: "strand",
      header: "Strand",
      width: 70,
      align: "right",
      cell: (m) => (
        <span
          className={`font-mono ${
            m.strand === "+" ? "text-foreground" : "text-amber-500"
          }`}
        >
          {m.strand}
        </span>
      ),
      sortValue: (m) => m.strand,
    },
  ];
}

function ModeTab({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`-mb-px border-b-2 px-3 py-1.5 text-xs font-medium transition-colors ${
        active
          ? "border-primary text-foreground"
          : "border-transparent text-muted-foreground hover:text-foreground"
      }`}
    >
      {children}
    </button>
  );
}

function downloadJson(data: unknown, name: string) {
  const blob = new Blob([JSON.stringify(data, null, 2)], {
    type: "application/json",
  });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  URL.revokeObjectURL(url);
}

function shortIri(iri: string | null | undefined): string {
  if (!iri) return "—";
  const match = iri.match(/[#/]([^#/]+)$/);
  return match ? match[1] : iri;
}
