# sbol-db documentation

Entry points to the project's documentation, organized by topic.

## Getting oriented

- **[Crate guide](crate-guide.md)**: architectural tour covering
  workspace layout, the storage model, import pipeline, query
  primitives, and key decision points. **Start here if you're new to
  the codebase.**

## Query primitives

`sbol-db` exposes three composable ways to read what you've imported:

- **[SPARQL endpoint](sparql.md)**: read-only SPARQL 1.1 evaluated
  directly against `sbol_quads`. SELECT, ASK, CONSTRUCT, and DESCRIBE
  are supported; SPARQL Update is rejected. Postgres remains the
  single source of truth, with no second index to operate.
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

- **[Postgres schema](schema.md)**: table-by-table reference for the
  embedded migrations. Documents, objects, the quad store, typed
  projections, validation, projection events, blank-node handling,
  index choices.

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
