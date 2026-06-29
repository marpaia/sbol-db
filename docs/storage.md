# Storage architecture

`sbol-db` keeps its persistence layer behind a backend-neutral contract.
Everything above the contract — the import pipeline, the five query primitives,
the SPARQL engine, the REST API, the CLI, the lab UI — depends only on a set of
traits, never on a concrete database. Three engines implement those traits
today: Postgres, SQLite, and RocksDB. You pick one with a connection string;
nothing else in the system changes.

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

The trait names a database nowhere. `sbol-db-storage` depends on
`sbol-db-core` for domain types and on nothing else.

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
`DATABASE_URL`), which defaults to the dev Postgres at
`postgres://sbol:sbol@localhost:5432/sbol`. An optional `--backend` flag (env
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

**Postgres** is the default and the most capable. Pick it for multi-node
deployments (several `sbol-db server` pods sharing one database, work
distributed by `FOR UPDATE SKIP LOCKED`), for the live sessions/locks and
slow-query views on the maintenance page, and when you want the
validation-findings audit trail and typed projection tables. It is the only
engine that runs as a separate server process; the others are embedded. See
[deployment.md](deployment.md) for the production shapes.

**SQLite** is a single-file, embedded SQL engine. It needs no server process and
no configuration beyond a file path. Being a SQL engine, it drives the lab's
SQL console and relational schema browser, and its maintenance page reports
table and index sizes with a VACUUM/ANALYZE action. Pick it for development, for
small single-node deployments, for embedding sbol-db inside another tool, and
for test fixtures where a throwaway database per test is convenient.

**RocksDB** is an embedded, single-node key/value engine with a triplestore
built directly on its column families. It stores each RDF term once under a
content-addressed 16-byte id and keeps permuted indexes so a triple pattern with
bound leading positions becomes one prefix scan. It has no SQL, so the lab shows
an LSM maintenance view instead of the SQL console: column-family and per-level
sizes, estimated keys, pending compaction, and a manual compaction action. Pick
it for single-node workloads that want the throughput of an in-process store and
the id-native SPARQL path, without a database server.

## The conformance suite

`crates/sbol-db-conformance` is the contract's executable definition. Each
scenario drives a backend purely through the trait surface and asserts the
behavior every engine must share: import and derived-view reads, the graph
set-semantics rule, neighborhood traversal, sequence search, ontology load and
closure, and the job-queue lifecycle. `run_all` runs them in sequence against
one store.

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
