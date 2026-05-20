# Sequence search

The sequence primitive answers "where does this nucleotide pattern
appear?" across every indexed `sbol:Sequence` in the database, with
reverse-complement-aware matching. It is the right surface for
restriction-site finding, exact primer matching, and motif lookup --
shapes that are awkward to express in SPARQL and pointless to express
in typed-IRI lookup.

The k-mer seed index sits beside the typed `sbol_sequences` projection
and is maintained automatically by the import pipeline. There is no
external `blastn`; the implementation is a few hundred lines of Rust
plus one Postgres table.

## CLI

```sh
sbol-db query sequence-search <pattern>
  [--max-hits 1024]
  [--forward-only]
```

```sh
# Find every occurrence of the EcoRI site (forward + RC).
sbol-db query sequence-search GAATTC

# Exact primer match on the forward strand only.
sbol-db query sequence-search ATGGCAGCAGCC --forward-only

# Longer motif; the seed index dominates the cost.
sbol-db query sequence-search CCAGGCATCAAATAAAACGAAAG
```

Output is JSON: an array of `{sequence_iri, start, length, strand}`,
where `strand` is `+` for forward matches and `-` for reverse-complement
matches. `start` is 0-indexed against the stored `elements` string.

Use `sbol-db query sequence-batch <path-or-->` to run many patterns
in one shot. The command reads newline-delimited patterns from a file
or stdin and emits one JSON object per line keyed by query.

## HTTP

```http
GET /sequences/search?pattern=<urlencoded>
  &max_hits=1024
  &forward_only=false
```

Returns the same JSON shape as the CLI. `pattern` is required;
`max_hits` defaults to 1024 and `forward_only` defaults to `false`.

## What gets indexed

A row in `sbol_sequences` participates in the search if all three hold:

- `elements` is non-null (the sequence string itself was imported);
- `alphabet` is `DNA` or `RNA` (the typed projection's alphabet column
  drives this -- protein sequences are excluded in v1);
- the string is at least `K` bases long (`K=8`).

Reindexing happens during `SbolObjectService::import_document`. When a
sequence is upserted, the prior k-mer rows are dropped and the new set
is bulk-inserted in the same transaction -- the seed index can never
drift from the typed projection.

## Match semantics

The search is **exact** -- no mismatches, no gaps, no degenerate IUPAC
expansion. Ambiguous bases (`N`, `R`, `Y`, ...) in the *query* will
fail to match anywhere; ambiguous bases in *indexed sequences* simply
interrupt the k-mer stream around them, so a query of all-canonical
bases that straddles an ambiguous position in the target will not hit
through that gap.

Reverse-complement matching is on by default. Pass `--forward-only`
(CLI) or `forward_only=true` (HTTP) to restrict to the forward strand.

## Architecture

`sbol_sequence_kmers` is the seed index: one row per (sequence,
position, strand) for every 8-mer in every indexed sequence. The
stored `kmer` is the *canonical* form -- `min(forward, reverse_complement)`
of the 2-bit packed nucleotide -- and `strand` records which arm of
the pair generated it. This halves the index footprint and turns
reverse-complement search into a single index probe.

At query time the engine:

1. Normalises the pattern (whitespace stripped, uppercased).
2. Computes the canonical k-mer of the first `K` bases of the pattern
   and of its reverse complement. (Both reduce to the same canonical
   if the pattern is its own reverse complement.)
3. Resolves candidate sequence ids via `sbol_sequence_kmers.kmer = ANY(seeds)`.
4. Fetches the candidates' `elements` column and verifies each
   candidate by direct substring match against the forward pattern and,
   unless `forward_only`, against the reverse complement.

For patterns shorter than `K=8` the seed step is skipped and the
candidate set is built via a bounded `UPPER(elements) LIKE '%P%'`
scan on `sbol_sequences`. This is intentionally simple and bounded:
`sbol_sequences` is small relative to `sbol_quads` and short-pattern
search is dominated by the restriction-site use case (6 bp).

## Performance

Indexing cost: roughly `O(length)` per sequence. A 10 kb part adds
~10 k rows to `sbol_sequence_kmers`. Bulk-insert via `UNNEST` keeps it a
single round trip per sequence.

Query cost: index probe on `sbol_sequence_kmers (kmer)` selects candidate
sequences quickly; the per-candidate verification reads `elements`
and runs Rust's `str::match_indices`, which is linear in the elements
length per candidate. For typical SBOL designs (hundreds of
sequences totalling <1 MB) a query completes in low single-digit
milliseconds.

## Errors

The endpoint never errors on "no match" -- an empty array is a
legitimate result. Truly empty input (a pattern containing only
whitespace) returns `[]` rather than scanning the entire database.

## What's intentionally not here

- **Approximate match.** Up to `N` mismatches / indels would require
  seed-and-extend with verification under an edit-distance budget.
  Deferred; the v1 scope is exact substrings with reverse-complement
  awareness.
- **Protein search.** `sbol_sequences.alphabet = 'PROTEIN'` is
  ignored. Protein k-mer indexing would need a larger alphabet
  (20-symbol) and a different `K`.
- **Position-weight matrix / regex motifs.** Out for v1. The
  neighborhood traversal + sequence search composition handles "find
  every promoter feature, then look for TATA in its sequence" well
  enough as two passes from a client.
- **Cross-sequence alignment.** Not in scope. `sbol-db` is a query
  database; a full alignment is a tool, not a primitive.
