# sbol-db crate guide

This guide orients a newcomer to the codebase. It complements:

- The [README](../README.md): quickstart and project scope.
- [`sparql.md`](sparql.md): the SPARQL endpoint in depth.
- [`neighborhood.md`](neighborhood.md): graph traversal API.
- [`sequences.md`](sequences.md): nucleotide substring + RC search.
- [`ontology.md`](ontology.md): ontology loading and term closure.
- [`schema.md`](schema.md): Postgres schema reference.

Read this first to know *where* things live and *why* the workspace is
shaped the way it is. Read the others when you need depth on a
particular surface.

## Scope

`sbol-db` is a database for synthetic biology data. Its query surface
runs over a pluggable storage engine (Postgres by default, with a
RocksDB engine coming soon). It is **not** a
DBTL workflow tracker. Projects, cycles, predictive model runs,
builds, samples, measurements, and decision records are
intentionally **out of scope** -- the goal is a best-in-class
query surface for SBOL itself, not the surrounding lab pipeline.

The focused scope is:

- Ingest SBOL 3 RDF, upgrade SBOL 2 RDF, and import GenBank or FASTA
  through the same document pipeline.
- Preserve the document losslessly (per-object JSON-LD slice + the
  full triple set under a named graph).
- Project the design into typed relational tables for SQL-shaped
  queries.
- Project the design into RDF triples for graph-shaped queries.
- Expose five composable query primitives: typed IRI lookup,
  bounded graph neighborhood, read-only SPARQL, nucleotide
  sequence search, and ontology-aware role expansion.
- Surface the same operations via a thin REST API and a CLI.

This is enough surface for "show me every component of role `X`",
"walk the structural decomposition from this design", "find every
design that references this part", "run an arbitrary SPARQL query
against the dataset". It's deliberately not enough surface for "track
the build state of this design".

## Workspace layout

Eight crates:

| Crate              | Purpose                                                                                |
| ------------------ | -------------------------------------------------------------------------------------- |
| `sbol-db-core`     | Domain types (`Triple`, `IriString`, `GraphId`, `NeighborhoodQuery`, …), the k-mer encoder, the OBO parser. No I/O. |
| `sbol-db-storage`  | Backend-neutral storage contract: the `SbolStore` / `TripleSource` / `JobQueue` traits and their request/response types. Names no concrete database. |
| `sbol-db-rdf`      | `sbol::Document` ↔ triples projection, RDF (re-)serialization, content hashing.           |
| `sbol-db-postgres` | Postgres implementation of the storage contract: sqlx repositories, embedded migrations, the `SbolObjectService` entry point. Hosts the sequence-search + ontology repositories.    |
| `sbol-db-sparql`   | Read-only SPARQL evaluator (`spareval::QueryableDataset` over any `TripleSource`).        |
| `sbol-db-ui`       | Embedded data-lab SPA served at `/lab` (React + Vite, baked in via `rust-embed`).        |
| `sbol-db-server`   | axum HTTP API. Thin layer over the storage + sparql crates.                             |
| `sbol-db`          | CLI binary. Wires the postgres + server + sparql crates into clap subcommands.          |

The boundaries matter:

- `sbol-db-core` has no `sqlx`, no `axum`, no `tokio` types in its
  public surface. Domain logic that doesn't need I/O lives here so
  it can be tested without a database.
- `sbol-db-storage` defines the persistence contract every backend
  satisfies. It names no concrete database, only traits and the
  request/response types they exchange. A new backend (RocksDB) is a
  new crate that implements these traits; nothing above the contract
  changes.
- `sbol-db-postgres` is the only crate that talks to sqlx. It
  implements the storage traits over a repository layer
  (`GraphRepository`, `SbolObjectRepository`, `TripleRepository`,
  `NeighborhoodRepository`, `TypedProjectionRepository`,
  `ValidationRepository`, `ProjectionEventRepository`).
- `sbol-db-sparql` reaches storage only through the `TripleSource`
  trait. SPARQL evaluation never sees sqlx types, migrations, or any
  concrete backend.

## Storage model

This section describes the default Postgres backend's layout; the
contract it satisfies is the `sbol-db-storage` traits, and another
backend is free to lay the same data out differently.

Three Postgres tables carry the bulk of the data:

- `sbol_graphs`: one row per named graph. A `kind = 'sbol3'` graph is a
  single imported SBOL document with import metadata (source URI,
  content hash, serialization format); a `kind = 'verbatim'` graph is
  raw RDF stored as written. The graph owns its triples.
- `sbol_objects`: one row per top-level SBOL object, keyed by IRI.
  Carries `sbol_class`, `display_id`, `name`, the JSON-LD slice for
  the object, `types`/`roles` arrays (for indexed filtering), and a
  content hash. This is the canonical KV store keyed by IRI.
- `sbol_triples`: one row per RDF triple. Carries graph IRI, subject /
  predicate / object positions, literal datatype + language, and a
  back-reference to the owning graph. Blank nodes live in
  companion `subject_blank` / `object_blank` text columns because
  the `sbol_iri` domain rejects `_:b0`-shaped values; see
  [`schema.md`](schema.md) for the full schema.

Typed projection tables (`sbol_components`, `sbol_sequences`,
`sbol_features`, `sbol_locations`, `sbol_constraints`,
`sbol_interactions`, `sbol_participations`) mirror the subset of
SBOL that benefits from columnar query: per-component types/roles,
sequence lengths and alphabets, feature parents, location ranges,
etc. These projections are derived during import from the
`sbol::Document` typed-objects iterator and never written by hand.

Validation findings and per-object revisions live in
`sbol_validation_runs` / `sbol_validation_findings` /
`sbol_object_revisions`. Projection events go to
`sbol_rdf_projection_events` (currently write-only — reserved for a
future async projection worker).

## Document lifecycle

The common flow is read → validate → persist → query.

### Read

`sbol-db` first normalizes imports to a native SBOL 3
`sbol::Document`. SBOL 3 RDF uses `sbol::Document::read(input,
RdfFormat::*)`; SBOL 2 RDF is detected by the SBOL 2 namespace and
upgraded with `Document::upgrade_from_sbol2_with`; GenBank and FASTA
go through `sbol-genbank` and `sbol-fasta`. The CLI infers the format
from the file extension, while the HTTP endpoint takes a Content-Type
or a `?format=` query parameter.

### Validate

The import service calls `Document::validate()` and writes the
report — passed or failed — into `sbol_validation_runs` and
`sbol_validation_findings`. A failed validation does not abort the
import; the document is still persisted so the caller can inspect
the findings. To gate hard on validation failures, surface
`validation_status` from the `ImportReport` and reject the import on
the caller side.

### Persist

`SbolObjectService::import_document` runs the full pipeline in one
Postgres transaction:

1. Insert the `sbol_graphs` row (`kind = 'sbol3'`).
2. For each `SbolObject` in `document.typed_objects()`:
   - Build a `SbolObjectRecord` (IRI, class, display ID, name,
     types, roles, the per-object JSON-LD slice, content hash).
   - Upsert into `sbol_objects`.
   - Write a row to `sbol_object_revisions`.
3. Project the document to triples (`document_to_triples`), tag every
   triple with `graph:document:{document_id}`, and replace the
   document's existing named graph atomically.
4. Run the typed projection inserts.
5. Record the validation run + findings.
6. Append a `document_imported` projection event.

The transaction boundary is deliberate: a partial import is never
visible. The projection event log is appended in the same
transaction so any future async consumer can rebuild derived state
deterministically.

### Query

Three query primitives, each scoped to a different shape of question:

**Typed IRI lookup** — "give me the object at this IRI". One indexed
row fetch from `sbol_objects`. Returns the lossless per-object
JSON-LD payload plus metadata.

```rust,ignore
let obj = svc.objects().get_by_iri(iri).await?;
```

**Graph neighborhood** — "what's near this IRI in the design
graph?". A bounded recursive CTE over `sbol_triples`, with optional
predicate allowlist, max-node cap, and forward / backward / both
direction. Returns nodes (decorated with sbol_class / displayId)
plus the visited edges. Optionally re-serializes the visited
subgraph as RDF. See [`neighborhood.md`](neighborhood.md).

```rust,ignore
let result = svc.neighborhood().walk(&NeighborhoodQuery { /* ... */ }).await?;
```

**SPARQL** — "express it as a SPARQL query". The evaluator runs
directly against `sbol_triples` via a `spareval::QueryableDataset`
implementation; no second index. SELECT / ASK / CONSTRUCT / DESCRIBE
are supported; SPARQL Update is rejected. See
[`sparql.md`](sparql.md).

```rust,ignore
let outcome = engine.execute(query_str, format, &options).await?;
```

**Sequence search** — "where does this nucleotide pattern appear?".
A k-mer seed index over `sbol_sequences.elements` plus a verification
pass against forward + reverse-complement. Maintained in lockstep
with the typed projection during import. See [`sequences.md`](sequences.md).

```rust,ignore
let hits = svc.sequence_search().search(pattern, options).await?;
```

**Ontology expansion** — "match every subclass of role X, not just
the literal IRI". OBO files (SO, SBO, ...) load from canonical URLs;
the transitive `is_a` closure is precomputed and exposed via
`OntologyRepository::descendants`. See [`ontology.md`](ontology.md).

```rust,ignore
let descendants = svc.ontology().descendants("SO:0000167").await?;
```

For typed columnar filtering (`SELECT * FROM sbol_components WHERE
'http://...promoter' = ANY(roles)`), the typed projection tables are
the right surface. They aren't exposed through a domain API at
present; the `TypedProjectionRepository` carries them. SQL is not a
public surface for end users — read the projections through the
relevant repository if you're consuming `sbol-db` as a library.

### Bulk shapes

Each query primitive has a plural form for callers that need to
amortise round-trips:

- **Bulk IRI lookup** — `SbolObjectRepository::get_by_iris(&[&str])` /
  `POST /objects/lookup`. One `WHERE iri = ANY($1)` scan; capped at
  1000 IRIs per call.
- **Corpus listing** — `SbolObjectRepository::list(&ListObjectsFilter)`
  / `GET /objects/list` / `sbol-db object export-all`. Keyset cursor on `iri`,
  page size capped at 5000; filters by `sbol_class`, `role`, and
  `graph_id` compose.
- **Bulk sequence search** — `SequenceSearchRepository::search_many` /
  `POST /sequences/search`. Loops over patterns; capped at 256 per call.
- **Atomic bulk import** — `SbolObjectService::import_documents` /
  `POST /graphs/bulk` / `sbol-db graph import <dir>` (default). The entire
  batch runs inside one Postgres transaction; either every document
  commits or none do. The HTTP endpoint caps at 100 documents per call;
  the CLI directory walker has no hard cap. Use this whenever the batch
  is a coherent unit and partial-import state is unacceptable.
- **Per-file directory import** — `sbol-db graph import <dir> --continue-on-error`
  is the escape hatch: each file runs in its own transaction, failures
  are logged and reported but don't abort the batch, and `--parallel N`
  enables concurrent imports. This is the right shape for corpus-scale
  onboarding where one bad TTL shouldn't roll back the other 999.
  `--parallel > 1` requires `--continue-on-error` since atomic batches
  are single-threaded by definition.

## Key decision points

These are the choices a newcomer hits first.

### Three layers, three projections

A single SBOL design lives in three representations after import:

- `sbol_objects.data` — lossless per-object JSON-LD slice. The
  authoritative payload for "give me this object back".
- typed projection tables — columnar view for SQL-shaped filters.
- `sbol_triples` — RDF view for graph-shaped queries (neighborhood,
  SPARQL, RDF export).

This is intentional duplication, not drift. Every projection is
derived from `sbol::Document` during import inside one transaction,
and a re-import deterministically replaces all three. The
`content_hash` columns provide a cheap drift check.

### SPARQL over the storage contract

There is no sidecar SPARQL store. `sbol-db-sparql` implements
`spareval::QueryableDataset` against a `TripleSource` and translates
each triple-pattern lookup to a single backend scan. The default
Postgres backend services that scan with an indexed SQL query against
`sbol_triples`; a future backend implements the same trait. The
trade-off: queries are always strongly consistent with committed
writes, but evaluation makes per-pattern round-trips. For the
workloads `sbol-db` targets (SBOL designs, tens of millions of
triples, exploratory queries from a human-in-the-loop), this is a
deliberate choice. See [`sparql.md`](sparql.md) for the perf
characteristics.

### Blank nodes alongside the `sbol_iri` domain

The `sbol_iri` Postgres domain enforces `^[a-zA-Z][a-zA-Z0-9+.-]*:.+`,
which rejects `_:b0`. Real SBOL documents (especially after a
parser pass) routinely carry blank-node resources in subject and
object positions. `sbol_triples` resolves this with companion
`subject_blank` / `object_blank` `text` columns plus a CHECK
constraint that exactly one of the (IRI, blank) pair is non-null
per position. `sbol_objects` is IRI-only — top-level SBOL objects
must have IRIs per spec, and the parser surfaces this as a
validation error if violated.

### Named-graph policy

Every triple inserted by the import pipeline is tagged with
`graph:document:{document_id}` as its graph IRI. This:

- gives RDF queries provenance (every triple is attributable to a
  specific document import);
- lets `replace_document_graph` atomically swap in the new triples on
  re-import without touching unrelated documents;
- lets SPARQL queries scope to a single document with
  `FROM NAMED <graph:document:…> WHERE { GRAPH <…> { … } }`.

For the SPARQL endpoint, the default graph is configured as the
union of all named graphs so plain `SELECT ?s WHERE { ?s ?p ?o }`
returns data without forcing the caller to know about the
named-graph policy. See [`sparql.md#named-graph-semantics`](sparql.md#named-graph-semantics).

### Where the integration tests live

Each crate that needs a live database has its own
`tests/<feature>_test.rs` that takes a `DATABASE_URL`, applies
migrations, truncates tables, imports a fixture, and exercises the
feature end-to-end. The fixtures (`simple_component.ttl`,
`nested_construct.ttl`, `invalid.ttl`) live under
`crates/sbol-db-postgres/tests/fixtures/` and are referenced by
sibling crates via relative `include_str!`.

This is by design: each crate's tests cover its public surface, no
test reaches across crate boundaries for plumbing, and the live
Postgres in `docker-compose.yaml` is the only dependency.

## Local development

`docker compose up -d` boots a Postgres 17 container at
`localhost:5432` with credentials `sbol/sbol/sbol`. The CLI defaults
to this connection string; export `DATABASE_URL` to override.

`sbol-db db migrate` applies pending migrations. The
migrator is `sqlx::migrate!` pointed at
`crates/sbol-db-postgres/migrations/`, which is the only place
migrations live.

`cargo test --workspace` runs the full suite. The integration tests
target the docker-compose Postgres; CI brings up a service container
of the same image. There is one `DB_MUTEX` per integration crate
that serializes table truncation across that crate's tests to
prevent cross-test contamination.

`cargo clippy --workspace --all-targets -- -D warnings` is the lint
gate; it's clean on `main`.

## What's intentionally not here

- **Background workers.** The `sbol_rdf_projection_events` table is
  written but no consumer processes it. A future async projection
  worker (Oxigraph sidecar, materialized view refresh, search index)
  would tail this log, but the v1 query surface is fully synchronous.
- **DBTL / lab / workflow tables.** See [Scope](#scope).
- **Sequence alignment, embeddings.** Out of scope for the v1
  surface. Exact-match sequence search with reverse-complement
  awareness ships via the k-mer index — see
  [`sequences.md`](sequences.md). Approximate alignment, embeddings,
  and richer full-text search are deferred.
- **SBOL 1 ingest.** SBOL 2 RDF, GenBank, and FASTA enter through the
  normal document import path. SBOL 1 remains deferred until there is
  an explicit converter and test corpus.
- **Multi-tenancy / auth.** No `organization_id` columns, no auth
  middleware. Repositories should still avoid global mutable state
  so a tenancy layer can be added without rewriting the data access
  path.
