# sbol-db

`sbol-db` is a database for synthetic biology data. It stores your
[SBOL](https://sbolstandard.org/) designs and lets you query them by
IRI, across the design graph, with SPARQL, or by DNA sequence.

It ingests SBOL 3 RDF, upgrades SBOL 2 RDF, and imports GenBank or
FASTA into SBOL 3, projecting every design into both a typed
relational schema and an RDF triplestore. The query surface is five
composable primitives: typed lookup by IRI, bounded graph neighborhood
traversal, read-only SPARQL 1.1, nucleotide substring +
reverse-complement search, and ontology-aware role expansion.

The persistence layer sits behind a backend-neutral storage contract
(the `sbol-db-storage` crate), so the same query surface runs on
Postgres, SQLite, or an embedded RocksDB engine. The connection-string
scheme picks the backend; Postgres is the default. See
[**docs/storage.md**](docs/storage.md).

<p align="center">
  <img src="docs/images/overview.png" alt="SBOL Data Lab overview dashboard showing corpus stats, top SBOL classes, and recent graphs" width="900">
</p>

New to the codebase? Start with the [**crate guide**](docs/crate-guide.md).
Want to see how a design flows through the tables? See the
[**domain model**](docs/domain-model.md). Deploying it? See
[**docs/deployment.md**](docs/deployment.md).

## Scope

`sbol-db` is deliberately narrow: a *best-in-class SBOL query
database*. It is not a DBTL workflow tracker, lab orchestration system,
or model registry. Designs are first-class; experiments, builds,
predictive model runs, and decision records are out of scope. The
focus is on sophisticated ways to query SBOL objects, not broader
project state.

Built on:

- [`sbol-rs`](https://github.com/marpaia/sbol-rs) for SBOL parsing,
  validation, and RDF I/O.
- Postgres, SQLite, or RocksDB as the storage engine, each implementing
  the `sbol-db-storage` contract. The scheme in `--database-url` selects
  the engine; Postgres is the default. See [storage.md](docs/storage.md).
- The [Oxigraph](https://github.com/oxigraph/oxigraph) ecosystem
  (`oxrdf`, `spareval`, `spargebra`, `sparesults`) for SPARQL.

## Installation

Bring up the dev Postgres (one command; the schema is applied on first
CLI invocation):

```sh
docker compose up -d
```

Build and install the CLI, then apply migrations:

```sh
cargo install --path crates/sbol-db
sbol-db db migrate
```

## Quickstart — CLI

```sh
# Import a single document.
sbol-db graph import path/to/design.ttl

# SBOL 2 RDF is upgraded to SBOL 3 on import.
sbol-db graph import path/to/legacy-sbol2.xml

# GenBank and FASTA are converted to SBOL 3 on import.
sbol-db graph import path/to/design.gbk --namespace https://example.org/lab
sbol-db graph import path/to/sequences.fasta --namespace https://example.org/lab

# Import an entire directory as one atomic transaction (commits all or none).
sbol-db graph import path/to/designs/ --skip-existing

# Corpus-scale onboarding: per-file txs, parallel, tolerate bad files.
sbol-db graph import path/to/corpus/ --continue-on-error --parallel 4 --skip-existing

# Resolve an object by IRI.
sbol-db object get https://synbiohub.org/public/igem/i13504

# Stream every stored object as newline-delimited JSON (corpus dump).
sbol-db object export-all --sbol-class http://sbols.org/v3#Component > components.jsonl

# Re-emit a single object as RDF.
sbol-db object export <iri> --format turtle

# Walk the bounded forward/backward neighborhood of an IRI.
sbol-db query neighborhood <iri> --depth 2 --direction both

# Find every occurrence of an EcoRI site (forward + reverse complement).
sbol-db query sequence-search GAATTC

# Load the Sequence Ontology, then list its descendants of "promoter".
sbol-db ontology fetch so
sbol-db ontology descendants SO:0000167

# Run a SPARQL query from stdin.
echo 'PREFIX sbol: <http://sbols.org/v3#>
SELECT ?s WHERE { ?s a sbol:Component } LIMIT 10' \
  | sbol-db query sparql -

# Start the HTTP server.
sbol-db server
# Then visit http://127.0.0.1:8888/docs for the Scalar-rendered API
# reference, or http://127.0.0.1:8888/openapi.json for the raw schema.
```

`sbol-db --help` lists all subcommands.

## Quickstart — Library

The CLI wraps the `sbol-db-storage` contract: `SbolObjectService`
(the Postgres implementation of `SbolStore`) and
`sbol-db-sparql::SparqlEngine` (which evaluates over any `TripleSource`).
Both are usable as library types:

```rust
use sbol_db_core::SerializationFormat;
use sbol_db_postgres::{connect, run_migrations, SbolObjectService};
use sbol_db_sparql::{ResultFormat, SparqlEngine, SparqlOptions};
use sbol_db_storage::ImportInput;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = connect("postgres://sbol:sbol@localhost:5432/sbol").await?;
    run_migrations(&pool).await?;
    let svc = SbolObjectService::new(pool);

    svc.import_document(ImportInput {
        body: std::fs::read_to_string("design.ttl")?,
        format: SerializationFormat::Turtle,
        namespace: None,
        source_uri: Some("design.ttl".into()),
        document_iri: None,
        created_by: None,
        name: None,
        description: None,
    })
    .await?;

    let engine = SparqlEngine::new(svc.triple_source());
    let outcome = engine
        .execute(
            "PREFIX sbol: <http://sbols.org/v3#> \
             SELECT ?s WHERE { ?s a sbol:Component }",
            Some(ResultFormat::Json),
            &SparqlOptions::default(),
        )
        .await?;
    println!("{}", String::from_utf8_lossy(&outcome.payload.body));
    Ok(())
}
```

## Data Lab

`sbol-db server` also serves the **SBOL Data Lab**, an embedded web UI
at [http://127.0.0.1:8888/lab](http://127.0.0.1:8888/lab). It's a
single front end over the whole query surface: write SPARQL or SQL
against your corpus, browse the relational schema and RDF prefixes,
load ontology packs, and watch jobs and metrics. The compiled assets
are baked into the binary, so the lab ships wherever the server does.
See [`docs/ui.md`](docs/ui.md).

Read-only SPARQL 1.1, with a prefix and class sidebar, saved queries,
and history:

<p align="center">
  <img src="docs/images/sparql.png" alt="SBOL Data Lab SPARQL editor with query results" width="900">
</p>

Or query the relational projection directly in SQL, with a browsable
table list:

<p align="center">
  <img src="docs/images/sql.png" alt="SBOL Data Lab SQL editor with query results" width="900">
</p>

## REST surface

`sbol-db server` starts an axum server. Routes mirror the CLI:

| Method | Path                              | Purpose                                |
| ------ | --------------------------------- | -------------------------------------- |
| `POST` | `/graphs`                         | Import SBOL RDF, GenBank, or FASTA     |
| `POST` | `/graphs/bulk`                    | Atomic bulk import (≤ 100, one txn)    |
| `GET`  | `/graphs/{id}`                    | Graph metadata                         |
| `GET`  | `/objects?iri=...`                | Resolve a stored object by IRI         |
| `GET`  | `/objects/list`                   | Paginated corpus listing (keyset cursor) |
| `POST` | `/objects/lookup`                 | Bulk IRI → object resolution (≤ 1000)  |
| `GET`  | `/objects/{id}/rdf`               | Re-emit object subgraph as RDF         |
| `GET`  | `/objects/neighborhood`           | Bounded graph traversal (JSON)         |
| `GET`  | `/objects/neighborhood.rdf`       | Bounded graph traversal (RDF subgraph) |
| `GET`/`POST` | `/sparql`                   | Read-only SPARQL 1.1 endpoint          |
| `GET`/`POST` | `/sparql-auth`              | SPARQL 1.1 Update (Basic auth)         |
| `*`    | `/sparql-graph-crud-auth/`        | Graph Store HTTP Protocol (Basic auth) |
| `GET`  | `/sequences/search`               | Nucleotide substring + RC search       |
| `POST` | `/sequences/search`               | Bulk pattern search (≤ 256 patterns)   |
| `GET`/`POST` | `/ontology`                 | List / load ontologies                 |
| `GET`  | `/ontology/term`                  | Term metadata (resolves IRI aliases)   |
| `GET`  | `/ontology/descendants`           | Transitive closure for a term          |
| `POST` | `/jobs`                           | Enqueue an async job (returns id)      |
| `GET`  | `/jobs`                           | List recent jobs (filterable)          |
| `GET`  | `/jobs/{id}`                      | One job (status, result, error)        |
| `POST` | `/jobs/{id}/cancel`               | Cancel a queued or running job         |
| `GET`  | `/healthz`                        | Static liveness probe                  |
| `GET`  | `/readyz`                         | Postgres `SELECT 1` readiness probe    |
| `GET`  | `/metrics`                        | Prometheus metrics exposition          |
| `GET`  | `/docs`                           | Interactive API docs (Scalar UI)       |
| `GET`  | `/openapi.json`                   | OpenAPI 3.1 schema                     |

See [`docs/sparql.md`](docs/sparql.md) for the SPARQL Protocol shape,
[`docs/neighborhood.md`](docs/neighborhood.md) for traversal parameters,
[`docs/sequences.md`](docs/sequences.md) for the k-mer search, and
[`docs/ontology.md`](docs/ontology.md) for ontology loading.

`sbol-db` can also stand in for the Virtuoso triplestore behind
[SynBioHub](https://synbiohub.org): the `/sparql-auth` and
`/sparql-graph-crud-auth/` endpoints implement the authenticated write
surface SynBioHub expects, storing RDF verbatim. See
[`docs/synbiohub.md`](docs/synbiohub.md).

## Async batch processing

For corpus-scale imports and background work, `sbol-db` ships a
Postgres-backed async job runtime that distributes work across every
node in the cluster — no sidecar broker, no leader election, no extra
infra. Each `sbol-db server` pod embeds a worker by default; multiple
pods share the database safely via `FOR UPDATE SKIP LOCKED`.

- **`POST /jobs`** and `sbol-db jobs enqueue` for fire-and-poll bulk
  imports, including worker-side public HTTPS imports for remote SBOL,
  GenBank, and FASTA sources.
- **At-least-once delivery** with idempotency keys, exponential
  backoff, and a dead-letter queue.
- **Embedded or dedicated** workers — run `sbol-db server` everywhere,
  or split the API and worker fleets with `--no-worker` and
  `sbol-db worker`.
- **Observable** via Prometheus: queue depth, oldest-queued age,
  per-kind throughput and durations, worker heartbeats. See the
  [deployment guide](docs/deployment.md#metrics).

```sh
# Enqueue an import job (returns a UUID immediately).
sbol-db jobs enqueue import_document @payload.json \
  --idempotency-key=doc:42
sbol-db jobs enqueue import_remote_document @remote-payload.json

# Poll until done.
sbol-db jobs status <uuid>
```

See [`docs/deployment.md#async-job-runtime`](docs/deployment.md#async-job-runtime)
for deployment shapes (single-node, two-node HA, dedicated worker fleet)
and operator-surface details.

## Workspace layout

| Crate              | Purpose                                                                                |
| ------------------ | -------------------------------------------------------------------------------------- |
| `sbol-db-core`     | Domain types shared across the workspace. No I/O dependencies.                          |
| `sbol-db-storage`  | Backend-neutral storage contract: the `SbolStore` / `TripleSource` traits and their request/response types. Names no concrete database. |
| `sbol-db-rdf`      | `sbol::Document` ↔ triples projection, RDF export, content hashing.                       |
| `sbol-db-derive`   | Pure import plan builder: parse, derive triples and object summaries, validate. No database. |
| `sbol-db-postgres` | Postgres implementation of the storage contract: sqlx repositories, embedded migrations, the `SbolObjectService` entry point. |
| `sbol-db-sqlite`   | SQLite implementation of the storage contract: a single-file, embedded SQL engine. |
| `sbol-db-rocksdb`  | RocksDB implementation: an embedded, dictionary-encoded, permuted-index triplestore. |
| `sbol-db-backend`  | Backend factory: `Backend::open` routes a connection string to the engine its scheme selects. |
| `sbol-db-conformance` | Backend-neutral conformance suite every engine passes through the trait surface. |
| `sbol-db-sparql`   | Read-only SPARQL evaluator (`spareval::QueryableDataset` over any `TripleSource`).        |
| `sbol-db-jobs`     | Async job runtime — `JobHandler` trait, registry, worker, built-in handlers.            |
| `sbol-db-ui`       | Embedded data-lab SPA served at `/lab` (React + Vite, baked in via `rust-embed`).        |
| `sbol-db-server`   | axum HTTP API.                                                                          |
| `sbol-db`          | CLI binary.                                                                             |

The boundary between the storage backends and `sbol-db-sparql` is the
`TripleSource` trait in `sbol-db-storage`: SPARQL evaluation never
touches a concrete database, only the trait's pattern-scan method. That
seam is what lets SQLite and RocksDB serve the same query surface as
Postgres. See the [crate guide](docs/crate-guide.md) and
[storage.md](docs/storage.md) for details.
