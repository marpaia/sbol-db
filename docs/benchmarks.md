# Benchmarks

sbol-db serves the same triplestore HTTP surface as Virtuoso, the store behind
SynBioHub, so the two are directly comparable on the workload SynBioHub issues.
The [`bench/`](../bench/) harness loads one SBOL corpus into Virtuoso and all
three sbol-db backends (Postgres, SQLite, RocksDB) over the Graph Store
Protocol, then replays realized SynBioHub SPARQL queries against each, with
SynBioHub out of the loop. See [`bench/README.md`](../bench/README.md) to run
it; the numbers below are the captured run in
[`bench/results/bench-v011.json`](../bench/results/bench-v011.json)
(`ghcr.io/marpaia/sbol-db:v0.1.1`, 40 iterations after 8 warmups, 189-file /
81,488-triple / 16 MB corpus).

## Read-query latency

Median latency in milliseconds; lower is better. All four backends store the
identical 81,488 triples and return identical row counts per query.

| Workload | Virtuoso | Postgres | SQLite | RocksDB |
| --- | --: | --: | --: | --: |
| List collections | 4.1 | 2.1 | 2.0 | **1.2** |
| Count parts | 3.5 | 1.5 | 1.1 | **1.0** |
| Object metadata | 2.6 | 1.6 | 1.3 | **1.2** |
| Browse / search | 17.6 | 3.8 | **2.2** | 4.9 |
| Search count | 6.4 | 2.0 | 1.4 | **0.9** |
| Collection members | 16.8 | 4.8 | 5.2 | **2.5** |

sbol-db answers every read workload faster than Virtuoso on all three backends;
the two heaviest queries, faceted browse and collection-member listing, fall
from 17.6 and 16.8 ms on Virtuoso to single-digit milliseconds. RocksDB is
usually the fastest engine.

## Ingest

Wall time to load the 189-file corpus into a fresh graph:

| | Virtuoso | Postgres | SQLite | RocksDB |
| --- | --: | --: | --: | --: |
| Corpus ingest | **1.6 s** | 67.0 s | 54.3 s | 38.7 s |

Bulk load is where Virtuoso is far ahead: its dedicated loader ingests the
corpus in 1.6 s, against 39–67 s for the sbol-db backends, whose per-document
derivation and typed projection are not tuned for load throughput.

## Reading these numbers

- **Cross-engine is indicative.** Virtuoso ships only as an x86-64 image and
  ran under emulation on the Apple Silicon test host, while the sbol-db image is
  native; that makes Virtuoso's numbers conservative. The comparison *among* the
  three sbol-db backends is like-for-like.
- **Scale.** The corpus is 81,488 triples on one host with client-observed
  latency. sbol-db's SPARQL evaluator scans once per triple pattern without join
  fusion, so the read advantage is not guaranteed to hold on much larger corpora
  or on join-heavy queries; repository-scale corpora, memory footprint, and
  end-to-end SynBioHub latency are not measured here.
