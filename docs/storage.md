# Storage architecture

`sbol-db` keeps its persistence layer behind a backend-neutral contract.
Everything above the contract — the import pipeline, query primitives, SPARQL
engine, REST API, CLI, and Admin UI — depends only on focused traits, never on
a concrete database. Three engines implement those traits today: RocksDB,
SQLite, and Postgres. RocksDB is the primary and default local/edge runtime;
the SQL engines remain conforming alternatives.

This document covers the contract, how a backend is selected at runtime, what is
shared across engines versus specific to one, and which engine to choose. For
the on-disk layout of each engine, see the layout references:

- [Postgres schema](schema-postgres.md)
- [SQLite schema](schema-sqlite.md)
- [RocksDB layout](schema-rocksdb.md)

## The contract

`crates/sbol-db-storage/src/traits.rs` defines the traits a backend implements.
They split persistence into focused surfaces:

| Trait | What it covers |
| --- | --- |
| `TripleSource` | Synchronous triple-pattern reads for the SPARQL evaluator. |
| `TripleWriter` | Atomic batch application of SPARQL-update changes. |
| `ObjectStore` | Derived-view object reads (by IRI, by id, listing). |
| `GraphStore` | Named-graph reads, deletion, and content-hash existence checks. |
| `CorpusStatsStore` | Constant-time exact counts over the canonical RDF catalog. |
| `NamedGraphCatalogStore` | Backend-independent graph and graph-triple keyset pages. |
| `ResourceCatalogStore` | Global RDF-resource identity, occurrences, filtering, and class counts. |
| `SequenceCatalogStore` | RDF-derived sequence metadata and keyset pages. |
| `OntologyStore` | Ontology loading, canonicalization, and closure queries. |
| `NeighborhoodStore` | Bounded graph-neighborhood traversal. |
| `SequenceSearchStore` | Nucleotide substring + reverse-complement search. |
| `SbolStore` | The umbrella read/ingest surface, composed of the five stores above plus `import_document` and the Graph Store write path. |
| `JobQueue` | Enqueue, lease-based dequeue, lifecycle transitions, and the operator read surface. |

`TripleSource` is the one synchronous trait. It backs the SPARQL evaluator's
`spareval::QueryableDataset`, which calls back into the dataset synchronously
once per triple pattern. A backend that is internally async (the SQL engines)
runs each scan to completion behind the sync method; an engine that is already
synchronous (RocksDB) serves it directly. The rest of the traits are
`#[async_trait]` and are held as `Arc<dyn ...>`.

The catalog traits are deliberately separate from `ObjectStore`. An SBOL-native
import can additionally materialize a typed object record, but a resource exists
because it occurs as a subject in canonical RDF. A verbatim SynBioHub graph and
a newly imported SBOL document therefore appear through the same Admin API.

The traits name a database nowhere. `sbol-db-storage` depends on
`sbol-db-core` for domain types and on nothing else.

## Canonical RDF and rebuildable projections

All three engines use the same structural model:

1. Canonical RDF triples grouped into named graphs are the source of truth.
2. `build_catalog_projection` derives occurrence-scoped resource metadata,
   class membership, collection membership, and sequence identity from those
   triples.
3. Each backend stores indexes suited to its engine. These indexes are
   rebuildable and commit atomically with ordinary graph writes.
4. Product and Admin list APIs page the universal catalog. They do not infer
   corpus contents from optional typed-object, typed-sequence, or import-history
   tables.

Resource identity is global by IRI, while metadata occurrences remain scoped to
their named graph. A global resource record deterministically merges those
occurrences and reports the exact graph count. This preserves graph-local facts
without showing the same identity as unrelated duplicate objects.

The dashboard reads maintained counters rather than running corpus-wide counts
at request time. Postgres and SQLite maintain a singleton counter row with
transactional triggers; RocksDB maintains reference counts and a versioned
catalog generation in the same `WriteBatch` as graph mutations. A legacy
RocksDB database with canonical data but no ready generation fails closed with
instructions to run `sbol-db db migrate`.

## Selecting a backend

`crates/sbol-db-backend` is the factory. `Backend::open(conn)` reads the scheme
off the connection string and routes to the matching engine:

| Scheme | Engine | Connection string |
| --- | --- | --- |
| `postgres://` or `postgresql://` | Postgres | `postgres://sbol:sbol@localhost:5432/sbol` |
| `sqlite://` | SQLite | `sqlite:///var/lib/sbol-db/sbol.db` |
| `rocksdb://` | RocksDB | `rocksdb:///var/lib/sbol-db/store.rocksdb` |

A scheme the factory does not recognize, or a string with no `://`, is a
startup error rather than a silent fallback.

The CLI and server take the connection string from `--database-url` (env
`DATABASE_URL`), which defaults to repo-local RocksDB at
`rocksdb://./.sbol-db/rocksdb`. For `sbol-db server`, the same default also
selects `.sbol-db/blobs` and `.sbol-db/text-index` unless their dedicated
environment variables override those paths. An optional `--backend` flag (env
`SBOL_DB_BACKEND`, one of `postgres` / `sqlite` / `rocksdb`) selects the engine
explicitly. When `--backend` is set it must agree with the URL's scheme, or the
URL may be a bare path that the backend completes into a scheme; a Postgres URL
paired with `--backend sqlite` is rejected so a connection string is never
opened as the wrong engine. The resolution logic is `resolve_connection` in
`crates/sbol-db/src/main.rs`.

```sh
# Scheme picks the engine.
sbol-db --database-url sqlite:///tmp/sbol.db graph import design.ttl
sbol-db --database-url rocksdb:///tmp/sbol.rocksdb server

# --backend completes a bare path.
sbol-db --backend sqlite --database-url /tmp/sbol.db db migrate
```

`Backend::open` returns a bundle of trait objects that every consumer shares:

```rust
pub struct Backend {
    pub kind: BackendKind,
    pub store: Arc<dyn SbolStore>,
    pub jobs: Arc<dyn JobQueue>,
    pub triple_source: Arc<dyn TripleSource>,
    pub triple_writer: Arc<dyn TripleWriter>,
    pub lab: Arc<dyn LabStore>,
    pub migrator: Option<Arc<dyn Migrator>>,
    pub db_stats: Option<Arc<dyn DbStats>>,
    pub lsm_stats: Option<Arc<dyn LsmStats>>,
    pub sql_console: Option<Arc<dyn SqlConsole>>,
    pub postgres: Option<PostgresHandle>,
}
```

The optional fields are capabilities a given engine may or may not provide.
Every engine has a migrator today. `db_stats` (relational engine
introspection: tables, indexes, schema, and on Postgres sessions and locks)
and `sql_console` (arbitrary SQL) are present for the SQL engines, Postgres and
SQLite. `lsm_stats` (column families, levels, and compaction) is present for
RocksDB. `postgres` is a typed handle to the pool for the things that really
are Postgres-specific: the dedicated worker pool, the connection-pool gauges,
and LISTEN/NOTIFY. `kind` lets the server report the engine and derive the lab
UI's capability flags. The lab serves `GET /lab/api/info` so the UI shows only
the features the running backend supports.

## What every backend shares, and what it does not

The import path is the clearest example of the shared/specific split. Parsing a
document, deriving its triples, building the object summaries, and validating it
are pure and backend-independent: `build_import_plan` in
`crates/sbol-db-derive/src/import.rs` turns an `ImportInput` into an
`ImportPlan` — the parsed document, the triples (each already tagged with the
minted graph IRI `graph:document:{id}`), the object summaries, the typed
projections, and the full validation report — without touching a database. Each
backend then commits that one plan atomically in its own idiom: Postgres and
SQLite in a SQL transaction, RocksDB in a single `WriteBatch`.

Because the derivation is shared, the engines agree on observable behavior. They
diverge in two places:

- How they store the data. Postgres uses relational tables and indexes; SQLite
  mirrors that model with portable types; RocksDB uses a dictionary-encoded,
  permuted-index key/value layout. The layout references document each.
- The engine-specific surfaces that are not part of the neutral contract. The
  SQL console and relational schema browser work on both SQL engines (Postgres
  and SQLite); RocksDB has no SQL, so it offers an LSM maintenance view
  (column families, levels, compaction) instead. Sessions, locks, and slow-query
  stats are Postgres-only refinements of the relational maintenance surface. The
  validation-findings audit trail and the typed projection tables
  (`sbol_components` and siblings) are Postgres-only; validation still runs on
  every engine during import and the status comes back in the `ImportReport`,
  but only Postgres persists the per-finding rows for later querying.

## Capability matrix

Every engine passes the full storage conformance suite (see below). The matrix
records that, plus the surfaces that differ by engine.

| Surface | Postgres | SQLite | RocksDB |
| --- | :---: | :---: | :---: |
| Universal RDF catalog (stats, graphs, resources, sequences) | yes | yes | yes |
| Document import + derived object view | yes | yes | yes |
| Graph set semantics | yes | yes | yes |
| SPARQL read (`TripleSource`) | yes | yes | yes |
| SPARQL update (`TripleWriter`) | yes | yes | yes |
| Graph-neighborhood walk | yes | yes | yes |
| Nucleotide sequence search | yes | yes | yes |
| Ontology load + transitive closure | yes | yes | yes |
| Async job queue | yes | yes | yes |
| SynBioHub query accelerator | yes | yes | yes |
| Id-native SPARQL scan | no | no | yes |
| SQL console (lab UI) | yes | yes | no |
| Relational schema browser + introspection (`DbStats`) | yes | yes | no |
| LSM maintenance + compaction (`LsmStats`) | no | no | yes |
| Sessions, locks, slow-query stats | yes | no | no |
| Validation-findings audit trail | yes | no | no |
| Typed projection tables | yes | no | no |

Two rows need a note. The id-native scan is a SPARQL-evaluation optimization:
RocksDB stores triples as term ids and can join on those ids, materializing
terms only for output rows (`supports_id_scan` returns `true`). Postgres and
SQLite use the term-materializing `scan_pattern` path against their indexes,
which is still index-backed and fast. The SynBioHub accelerator is the
per-graph derived index set that answers SynBioHub's fixed query templates with
point lookups instead of full graph-pattern evaluation; all three engines build
it.

## Choosing an engine

**RocksDB** is the primary default for a consolidated single-server deployment.
It is an embedded key/value engine with a triplestore built directly on its
column families. It stores each RDF term once under a content-addressed 16-byte
id and keeps permuted indexes so a triple pattern with bound leading positions
becomes one prefix scan. The Admin maintenance view reports LSM state instead
of pretending SQL, pool, lock, or relational-schema features exist. It is
single-process storage: do not put multiple servers in front of one directory.

**SQLite** is a single-file, embedded SQL engine. It needs no server process and
no configuration beyond a file path. Being a SQL engine, it drives the lab's
SQL console and relational schema browser, and its maintenance page reports
table and index sizes with a VACUUM/ANALYZE action. Pick it for development, for
small single-node deployments, for embedding sbol-db inside another tool, and
for test fixtures where a throwaway database per test is convenient.

**Postgres** is the shared-database choice for multi-process deployments
(several `sbol-db server` instances using one database, with jobs distributed by
`FOR UPDATE SKIP LOCKED`). Pick it when that topology or its live
sessions/locks, slow-query views, validation audit, and typed projection tables
are required. Those additional tables are conveniences, not a different corpus
definition.

## The conformance suite

`crates/sbol-db-conformance` is the contract's executable definition. Each
scenario drives a backend purely through the trait surface and asserts the
behavior every engine must share: import and derived-view reads, universal RDF
catalog counts and pagination, graph set semantics, neighborhood traversal,
sequence search, ontology load and closure, identity pagination, and the
job-queue lifecycle. `run_all` runs them in sequence against one store.

Every backend wires the suite into its own tests and passes the full set:
`postgres_passes_storage_conformance_suite`,
`sqlite_passes_full_conformance_suite`, and
`rocksdb_passes_full_conformance_suite`. The SQLite and RocksDB suites run
against a throwaway database with no external dependency:

```sh
cargo test -p sbol-db-sqlite -p sbol-db-rocksdb --test conformance_test
```

The Postgres suite needs a live database (`docker compose up -d postgres` and a
`DATABASE_URL`).

A new backend is a new crate that implements the `sbol-db-storage` traits and
passes this suite. Nothing above the contract has to change for it.
