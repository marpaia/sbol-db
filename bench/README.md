# Benchmarking sbol-db against Virtuoso

This harness measures the triplestore workloads SynBioHub issues, run directly
against Virtuoso and all three sbol-db storage backends (Postgres, SQLite,
RocksDB), with no SynBioHub in the loop, so the numbers reflect the database
itself.

`docker-compose.yml` brings the four services up side by side, each on its own
host port:

| Service | Backend | Port |
| --- | --- | --- |
| `virtuoso` | Virtuoso 7.2.5 | 18901 |
| `sboldb-postgres` | sbol-db on Postgres | 18902 |
| `sboldb-sqlite` | sbol-db on SQLite | 18903 |
| `sboldb-rocksdb` | sbol-db on RocksDB | 18904 |

`bench.py` loads the same SBOL corpus into each over the Graph Store Protocol,
lets each store settle, then replays realized versions of SynBioHub's own SPARQL
queries, recording the client-observed latency distribution per query and the
wall time to ingest the corpus. `gen_report.py` turns a results JSON into the
LaTeX fragments in `out/` and prints a Markdown summary.

## Prerequisites

- Docker. Virtuoso is pulled automatically; the sbol-db image defaults to the
  published `ghcr.io/marpaia/sbol-db:v0.1.1` (override with `SBOLDB_IMAGE`, e.g.
  a locally built `sbol-db:bench` from the repo root:
  `docker build -t sbol-db:bench .`). The image links glibc so the RocksDB
  backend's C++ library builds.
- Python 3 with `requests` (`pip install requests`, or point `PYTHON` at a
  virtualenv).
- A corpus directory of SBOL2 RDF/XML (`*.xml`) files (see below).

## The corpus

The harness loads any directory of SBOL2 RDF/XML files, passed with `--corpus`.
The captured run in `results/bench-v011.json` used a 189-file, 16 MB corpus
(81,488 triples): the SBOLTestSuite documents round-tripped through SynBioHub's
`SBOLTestRunner` so each is a self-contained SBOL2 submission. Any comparable
SBOL2 corpus reproduces the shape of the result; the exact latencies scale with
corpus size and content.

## Usage

```sh
bench/run-bench.sh --corpus /path/to/sbol2-rdfxml-dir      # full run, leaves the stack up
bench/run-bench.sh --corpus ./corpus --iterations 100
bench/run-bench.sh --corpus ./corpus --down                # tear the stack down afterward
```

Results are written to `results/bench-<host>.json` and LaTeX fragments to
`out/`. To benchmark a subset of backends, pass e.g. `--sboldb-sqlite skip` to
`bench.py`. Tear the stack down manually with:

```sh
docker compose -p sboldbbench -f bench/docker-compose.yml down -v
```

## Method

- **Identical data.** Every `.xml` file is POSTed as `application/rdf+xml` into
  the graph `http://synbiohub.org/public` on each backend. The run prints the
  loaded triple count per backend; all four agree (a graph is a set of triples
  on every backend), which confirms parity.
- **Identical queries.** The read queries are realized from SynBioHub's
  templates with `FROM` at the top level (valid SPARQL 1.1), so the same query
  string runs unchanged everywhere. Sample URIs are discovered from the loaded
  data so queries hit real objects. After timing, the run checks that every
  backend returned the same row count for each `SELECT` (`DESCRIBE` is excluded:
  its result scope is implementation-defined).
- **Settle before reading.** Each backend runs its maintenance pass (RocksDB
  compaction / SQL optimize) before the read measurements, so a freshly
  bulk-loaded, un-compacted index does not skew the numbers. Virtuoso has no
  such endpoint and is left as-is.
- **Timing.** Client-side wall time over `--iterations` timed calls after
  `--warmup` warmups; the JSON keeps mean, p50, p95, min, max, and throughput.
  A query that errors or exceeds `--query-timeout` is recorded as failed rather
  than aborting the run. Ingest is the wall time to load the whole corpus into a
  fresh graph, averaged over `--ingest-runs` runs.

## Architecture note

Virtuoso ships only as an x86-64 image and runs under emulation on Apple
Silicon, while the sbol-db image is native. That makes Virtuoso's numbers
conservative, so the cross-engine comparison is indicative; the comparison
*among* the three sbol-db backends is like-for-like. On a native x86-64 host the
absolute Virtuoso numbers would improve.

## Captured results

`results/bench-v011.json` is a full run against `ghcr.io/marpaia/sbol-db:v0.1.1`
with the defaults (40 iterations, 8 warmups, 3 ingest runs) over the 189-file
corpus, and `out/` holds the LaTeX fragments generated from it. See
[`docs/benchmarks.md`](../docs/benchmarks.md) for the rendered tables and
discussion.
