# sbol-db documentation

Entry points to the project's documentation, organized by topic.

## Getting oriented

- **[Crate guide](crate-guide.md)**: architectural tour covering
  workspace layout, the storage model, import pipeline, query
  primitives, and key decision points. **Start here if you're new to
  the codebase.**

## Query primitives

`sbol-db` exposes five composable ways to read what you've imported:

- **[SPARQL endpoint](sparql.md)**: read-only SPARQL 1.1 evaluated
  directly against the active backend's triples through the
  `TripleSource` contract. SELECT, ASK, CONSTRUCT, and DESCRIBE are
  supported; SPARQL Update is rejected. The store is the single source
  of truth, with no second index to operate.
- **[Graph neighborhood](neighborhood.md)**: bounded recursive
  traversal in either direction. Filter by predicate, cap by depth
  and node count, emit JSON or a self-contained RDF subgraph.
- **[Sequence search](sequences.md)**: nucleotide substring search
  with reverse-complement awareness, backed by a per-sequence k-mer
  seed index. Restriction sites, exact primers, motifs.
- **[Ontology expansion](ontology.md)**: load SO / SBO / others from
  canonical OBO URLs, precompute the transitive closure, and resolve
  identifier.org / OBO Foundry IRI aliases for role queries.
- **Object lookup**: typed resolution by IRI through
  `SbolObjectRepository::get_by_iri` / `GET /objects?iri=…`. Covered
  in the [crate guide](crate-guide.md).

## Storage

- **[Storage architecture](storage.md)**: the backend-neutral contract,
  how a backend is selected by connection-string scheme, what is shared
  across engines versus specific to one, the capability matrix, and the
  conformance suite. **Start here for the storage layer.**
- **[Postgres schema](schema-postgres.md)**: table-by-table reference for
  the default backend. Documents, objects, the triplestore, set
  semantics, typed projections, validation, the accelerator, index
  choices.
- **[SQLite schema](schema-sqlite.md)**: table-by-table reference for the
  single-file embedded SQL backend.
- **[RocksDB layout](schema-rocksdb.md)**: column-family, term-dictionary,
  and permuted-index reference for the embedded key/value backend.

## Operations

- **[Deployment guide](deployment.md)**: container image, CI workflows,
  Helm chart, environment-variable reference, probes, metrics, JSON
  logging, graceful shutdown semantics, capacity planning, and a
  troubleshooting playbook. Start here when standing sbol-db up in a
  real environment.

## Scope

`sbol-db` deliberately stays narrow on SBOL query capabilities and
does not expand into broader DBTL / workflow orchestration concerns.
The [crate guide](crate-guide.md#scope) spells out what's in and
what's out, with rationale.
