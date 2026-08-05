# SBOL DB

SBOL DB is open infrastructure for biological design data: a self-hosted
registry, an interoperable database, and an operator-ready server built around
the [Synthetic Biology Open Language](https://sbolstandard.org/). It gives
people one place to find, inspect, contribute, share, review, and reuse designs,
while giving software stable APIs and provenance-preserving representations of
the same records.

<p align="center">
  <img src="https://raw.githubusercontent.com/marpaia/sbol-db/master/docs/images/homepage.png" alt="SBOL DB registry homepage for finding, sharing, and reusing biological designs" width="900">
</p>

The browser application and the machine interfaces are two views of one
system. SBOL DB accepts SBOL 3, upgrades SBOL 2, and converts GenBank and FASTA
into SBOL 3. It preserves stable identities, graph structure, biological
meaning, and provenance while projecting each design into RDF and typed data
models. Users can discover records by metadata and biological facets, DNA
sequence, or configured semantic and related-design strategies; software can
reach the same corpus through REST, SPARQL, and the CLI.

One `sbol-db server` process can host the public registry, account and
collaboration workflows, administrator workspace, APIs, and background worker.
The default local runtime uses embedded RocksDB; SQLite and Postgres implement
the same storage contract for other deployment shapes. The self-contained
production profile adds native HTTPS, ACME certificate lifecycle, durable
configuration, scheduled encrypted backups, remote readback verification, and
offline atomic restore. See the [storage architecture](docs/storage.md) and
[deployment guide](docs/deployment.md).

## What it includes

- **A biological design registry.** Search, canonical object pages, typed
  relationships, provenance, attachments, collections, and truthful downloads
  live under one origin.
- **Contribution and collaboration.** Validate before committing, preview
  minted identities and conversion warnings, publish collections, share
  designs, transfer ownership, and run auditable curator reviews.
- **Biology-aware discovery.** Ranked text and facets, ontology-aware roles,
  exact and aligned sequence search, graph neighborhoods, SPARQL 1.1, and
  pluggable structured search strategies.
- **Interfaces for tools.** Native and V2 REST APIs, OpenAPI documentation, the
  `sbol-db` CLI, RDF and common biological formats, plus compatibility surfaces
  for existing SynBioHub clients.
- **An embedded control plane.** Inspect data, SPARQL, jobs, storage, search
  indexes, users, integrations, audit history, edge health, and complete
  backup evidence without deploying a separate admin application.
- **Compatibility and migration.** Run classic SynBioHub against SBOL DB's
  compatibility endpoints, measure behavior with the differential suite, and
  use the preflighted, reconciled migration path for the persistence surfaces
  covered by the [migration guide](docs/synbiohub-migration.md).

## Scope

SBOL DB follows the lifecycle of a _biological design record_: ingest,
validation, identity, discovery, contribution, publication, collaboration,
exchange, and operations. It is not a DBTL workflow tracker, lab orchestration
system, ELN, or model registry. Experiments, builds, samples, measurements,
predictive model runs, and decision records remain out of scope.

New to the codebase? Start with the [crate guide](docs/crate-guide.md) and
[domain model](docs/domain-model.md). Running a registry? Start with the
[application guide](docs/ui.md) and [deployment guide](docs/deployment.md).
For the full documentation map, see [docs/README.md](docs/README.md).

Core foundations:

- [`sbol-rs`](https://github.com/marpaia/sbol-rs) for SBOL parsing,
  validation, and RDF I/O.
- Postgres, SQLite, or RocksDB as the storage engine, each implementing
  the `sbol-db-storage` contract. The scheme in `--database-url` selects
  the engine; repo-local RocksDB is the default. See [storage.md](docs/storage.md).
- The [Oxigraph](https://github.com/oxigraph/oxigraph) ecosystem
  (`oxrdf`, `spareval`, `spargebra`, `sparesults`) for SPARQL.
- A zero-configuration, local ranked-text search index over stable SBOL object
  metadata. Explicit deployments can additionally configure the BGE-small
  vector-search index whose verified weights ship in the production image. See
  [search-plugins.md](docs/search-plugins.md) and
  [builtin-bge-small-model.md](docs/builtin-bge-small-model.md).

## Registry application and admin workspace

`sbol-db server` serves the public registry at
[http://127.0.0.1:8888/](http://127.0.0.1:8888/) and the administrator workspace
at `/admin`. Registry search, canonical records, contribution, collections,
accounts, collaboration, APIs, and operations share one origin and one domain
model. The compiled React assets are baked into the Rust binary, so the
application ships wherever the server does. SynBioHub is a compatibility and
migration boundary, not the product brand. See the [application guide](docs/ui.md)
and [application architecture](docs/portal-architecture.md).

Read-only SPARQL 1.1, with a prefix and class sidebar, saved queries,
and history:

<p align="center">
  <img src="https://raw.githubusercontent.com/marpaia/sbol-db/master/docs/images/sparql.png" alt="SBOL DB administrator SPARQL workspace with prefixes, saved queries, history, and results" width="900">
</p>

The same workspace exposes background jobs, search-index lifecycle, storage
maintenance, instance policy, users, integrations, audit events, production
edge health, and verified backup and recovery evidence. The
[application guide](docs/ui.md#a-tour-of-the-product) includes the complete
screenshot tour.

<p align="center">
  <img src="https://raw.githubusercontent.com/marpaia/sbol-db/master/docs/images/backups.png" alt="SBOL DB complete backup and recovery workspace with service health, remote verification, and active policy" width="900">
</p>

## Installation

Build and run the server. With no environment variables or CLI overrides it
uses `.sbol-db/rocksdb`, `.sbol-db/blobs`, and `.sbol-db/text-index` under the
current checkout; the ignored directory is created automatically when absent:

```sh
cargo build
./target/debug/sbol-db server
```

To install the CLI, run:

```sh
cargo install --path crates/sbol-db
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

The storage and SPARQL layers are also public Rust APIs. This narrow example
uses the Postgres `SbolObjectService` implementation of `SbolStore` with
`sbol-db-sparql::SparqlEngine`, which evaluates over any `TripleSource`:

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

See [`docs/sparql.md`](docs/sparql.md) for the SPARQL Protocol shape,
[`docs/neighborhood.md`](docs/neighborhood.md) for traversal parameters,
[`docs/search-plugins.md`](docs/search-plugins.md) for pluggable search,
[`docs/sequences.md`](docs/sequences.md) for the k-mer search, and
[`docs/ontology.md`](docs/ontology.md) for ontology loading.

`sbol-db` can also stand in for the Virtuoso triplestore behind
[SynBioHub](https://synbiohub.org): the `/sparql-auth` and
`/sparql-graph-crud-auth/` endpoints implement the authenticated write
surface SynBioHub expects, storing RDF verbatim. See
[`docs/synbiohub.md`](docs/synbiohub.md).

## Async batch processing

For corpus-scale imports and background work, `sbol-db` ships a durable async
job runtime over the selected storage backend. Each `sbol-db server` process
embeds a worker by default. Embedded SQLite and RocksDB deployments own their
queue in the single server; Postgres deployments can distribute work across
multiple API and worker nodes with `FOR UPDATE SKIP LOCKED`, without a sidecar
broker or leader election.

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
