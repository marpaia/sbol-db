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
 * Patterns shorter than 8 bp fall off the k-mer seed index onto an
 * `ILIKE` candidate scan, which can be much slower. The form surfaces
 * a hint when that happens so the user knows what they're paying for.
 */

import { useMemo, useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { Dna, Download, Loader2, Search, TriangleAlert } from "lucide-react";

import { DataTable, type DataTableColumn } from "@/components/lab/DataTable";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import {
  sequenceSearch,
  sequenceSearchBatch,
  type BatchSequenceMatch,
  type SequenceMatch,
} from "@/lib/api";
import { useLabStore } from "@/lib/store";
import { describeError } from "@/lib/utils";

const KMER_SEED_BP = 8;
const SINGLE_MAX_HITS_DEFAULT = 1024;
const BATCH_MAX_PATTERNS = 256;

type Mode = "single" | "batch";

export default function SequencesRoute() {
  const navigate = useNavigate();
  const recent = useLabStore((s) => s.recentSeqPatterns);
  const remember = useLabStore((s) => s.rememberSeqPattern);

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
    navigate(`/objects/${encodeURIComponent(iri)}`);

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-6 px-8 py-10">
        <header className="space-y-1.5">
          <div className="flex items-center gap-2">
            <Dna size={16} className="text-muted-foreground/70" />
            <h1 className="text-2xl font-semibold tracking-tight">
              Sequence search
            </h1>
          </div>
          <p className="text-sm text-muted-foreground">
            Exact-match nucleotide search across every indexed{" "}
            <code className="font-mono">sbol:Sequence</code>. Patterns ≥{" "}
            {KMER_SEED_BP} bp use the k-mer seed index; shorter patterns fall
            back to an <code className="font-mono">ILIKE</code> candidate scan.
          </p>
        </header>

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
                  : batchPatterns.length === 0 ||
                    tooManyBatch ||
                    batch.isPending
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
                index and will scan via <code className="font-mono">ILIKE</code>
                . Expect slower results on large corpora.
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
      </div>
    </div>
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
