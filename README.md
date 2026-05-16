![sbol-db: A Postgres-backed data management system for synthetic biology](docs/images/sbol-db.png)

`sbol-db` is a Postgres-backed data management system for
synthetic biology data. It ingests [SBOL 3](https://sbolstandard.org/)
documents in any spec-listed RDF serialization, projects them into
a typed relational schema *and* an RDF quad store inside the same
Postgres instance, then exposes the result through three composable
query primitives: typed lookup by IRI, bounded graph neighborhood
traversal, and read-only SPARQL 1.1.

New to the codebase? Start with the [**crate guide**](docs/crate-guide.md).
Deploying it? See [**docs/deployment.md**](docs/deployment.md).

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
- Postgres 17 as the canonical durable store.
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
sbol-db migrate up
```

## Quickstart — CLI

```sh
# Import a document.
sbol-db import path/to/design.ttl

# Resolve an object by IRI.
sbol-db get https://synbiohub.org/public/igem/i13504

# Re-emit a single object as RDF.
sbol-db export <iri> --format turtle

# Walk the bounded forward/backward neighborhood of an IRI.
sbol-db neighborhood <iri> --depth 2 --direction both

# Find every occurrence of an EcoRI site (forward + reverse complement).
sbol-db sequences search GAATTC

# Load the Sequence Ontology, then list its descendants of "promoter".
sbol-db ontology fetch so
sbol-db ontology descendants SO:0000167

# Run a SPARQL query from stdin.
echo 'PREFIX sbol: <http://sbols.org/v3#>
SELECT ?s WHERE { ?s a sbol:Component } LIMIT 10' \
  | sbol-db sparql -

# Start the HTTP server.
sbol-db serve
# Then visit http://127.0.0.1:8080/docs for the Scalar-rendered API
# reference, or http://127.0.0.1:8080/openapi.json for the raw schema.
```

`sbol-db --help` lists all subcommands.

## Quickstart — Library

The CLI is a thin wrapper around `sbol-db-postgres::SbolObjectService`
and `sbol-db-sparql::SparqlEngine`. Both are usable as library types:

```rust
use std::sync::Arc;
use sbol_db_core::SerializationFormat;
use sbol_db_postgres::{connect, run_migrations, ImportInput, SbolObjectService};
use sbol_db_sparql::{ResultFormat, SparqlEngine, SparqlOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = connect("postgres://sbol:sbol@localhost:5432/sbol").await?;
    run_migrations(&pool).await?;
    let svc = SbolObjectService::new(pool);

    svc.import_document(ImportInput {
        body: std::fs::read_to_string("design.ttl")?,
        format: SerializationFormat::Turtle,
        source_uri: Some("design.ttl".into()),
        document_iri: None,
        created_by: None,
        name: None,
        description: None,
    })
    .await?;

    let engine = SparqlEngine::new(Arc::new(svc.quads().clone()));
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

## REST surface

`sbol-db serve` starts an axum server. Routes mirror the CLI:

| Method | Path                              | Purpose                                |
| ------ | --------------------------------- | -------------------------------------- |
| `POST` | `/documents`                      | Import an SBOL document                |
| `GET`  | `/documents/{id}`                 | Document metadata                      |
| `GET`  | `/objects?iri=...`                | Resolve a stored object by IRI         |
| `GET`  | `/objects/{id}/rdf`               | Re-emit object subgraph as RDF         |
| `GET`  | `/objects/neighborhood`           | Bounded graph traversal (JSON)         |
| `GET`  | `/objects/neighborhood.rdf`       | Bounded graph traversal (RDF subgraph) |
| `GET`/`POST` | `/sparql`                   | Read-only SPARQL 1.1 endpoint          |
| `GET`  | `/sequences/search`               | Nucleotide substring + RC search       |
| `GET`/`POST` | `/ontology`                 | List / load ontologies                 |
| `GET`  | `/ontology/term`                  | Term metadata (resolves IRI aliases)   |
| `GET`  | `/ontology/descendants`           | Transitive closure for a term          |
| `GET`  | `/healthz`                        | Static liveness probe                  |
| `GET`  | `/readyz`                         | Postgres `SELECT 1` readiness probe    |
| `GET`  | `/metrics`                        | Prometheus metrics exposition          |
| `GET`  | `/docs`                           | Interactive API docs (Scalar UI)       |
| `GET`  | `/openapi.json`                   | OpenAPI 3.1 schema                     |

See [`docs/sparql.md`](docs/sparql.md) for the SPARQL Protocol shape,
[`docs/neighborhood.md`](docs/neighborhood.md) for traversal parameters,
[`docs/sequences.md`](docs/sequences.md) for the k-mer search, and
[`docs/ontology.md`](docs/ontology.md) for ontology loading.

## Workspace layout

| Crate              | Purpose                                                                                |
| ------------------ | -------------------------------------------------------------------------------------- |
| `sbol-db-core`     | Domain types shared across the workspace. No I/O dependencies.                          |
| `sbol-db-rdf`      | `sbol::Document` ↔ quads projection, RDF export, content hashing.                       |
| `sbol-db-postgres` | sqlx repositories, embedded migrations, the `SbolObjectService` domain entry point.     |
| `sbol-db-sparql`   | Read-only SPARQL evaluator (`spareval::QueryableDataset` over `sbol_quads`).            |
| `sbol-db-server`   | axum HTTP API.                                                                          |
| `sbol-db`          | CLI binary.                                                                             |

The boundary between `sbol-db-postgres` and `sbol-db-sparql` is the
`QuadRepository::scan_pattern` primitive: SPARQL evaluation never
touches sqlx directly, only the repository's pattern-scan method. See
the [crate guide](docs/crate-guide.md) for details.
