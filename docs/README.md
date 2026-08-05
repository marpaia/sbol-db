# sbol-db documentation

SBOL DB is open biological design infrastructure: a public and account-aware
registry, an interoperable SBOL database, a machine-facing API and CLI, and an
embedded operations plane. These entry points cover the product from browser
workflows through storage and production recovery.

## Getting oriented

- **[Crate guide](crate-guide.md)**: architectural tour covering
  workspace layout, the storage model, import pipeline, query
  primitives, and key decision points. **Start here if you're new to
  the codebase.**
- **[Application architecture and roadmap](portal-architecture.md)**: route
  ownership, browser/API compatibility, session and admin boundaries,
  frontend layering, design-system direction, and the phased SynBioHub
  compatibility migration.
- **[Application acceptance contract](application-acceptance.md)**: autonomous
  delivery boundaries, hard semantic/API/security/design gates, evidence tiers,
  and measurable exit criteria for every application phase.
- **[Domain model](domain-model.md)**: how SBOL documents, graphs, objects,
  identities, triples, sequences, and typed projections relate.
- **[SBOL DB Design Ledger](design-ledger.md)**: the product-owned visual
  identity, semantic SBOL palette, cross-surface rules, and UI acceptance
  checklist for the registry, administrator control plane, and API reference.

## Product workflows

- **[Application and admin UI guide](ui.md)**: the current public registry,
  account and collaboration workflows, admin control plane, screenshot tour,
  configuration switches, and local development loop.
- **[The SynBioHub v2 API](api-v2.md)**: the RESTful product contracts for instance bootstrap,
  sessions, accounts, objects, contribution, collections, collaboration,
  review, search, downloads, and administration.
- **[Self-contained edge deployment](edge-deployment.md)**: the one-server
  RocksDB production profile with native HTTPS, ACME, a managed data layout,
  encrypted complete backups, remote verification, and atomic offline restore.
- **[Migrating a classic instance](synbiohub-migration.md)**: preflight,
  import, reconciliation, and cutover from a deployed classic SynBioHub.

## Query primitives

The registry and collaboration workflows sit on biology-aware ways to read and
discover what has been imported:

- **[Registry discovery contract](discovery-contract.md)**: normalized text
  and biological facets, exact totals, deterministic paging, public URL state,
  and the explicit classic-link translation boundary.
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
- **[Search strategy SDK guide](search-sdk-guide.md)**: a shareable,
  idea-to-evaluation tutorial for implementing classic, embedding, neural,
  hybrid, agentic, or vector-engine search plugins in Rust.
- **[Pluggable search reference](search-plugins.md)**:
  compatibility-preserving strategy contracts, embedding providers, vector
  backends, generation-based index maintenance, deployment choices, and
  relevance evaluation gates.

## Storage

- **[Storage architecture](storage.md)**: the backend-neutral contract,
  how a backend is selected by connection-string scheme, what is shared
  across engines versus specific to one, the capability matrix, and the
  conformance suite. **Start here for the storage layer.**
- **[Postgres schema](schema-postgres.md)**: table-by-table reference for
  the server-oriented SQL backend. Documents, objects, the triplestore, set
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

## SynBioHub compatibility

- **[Compatibility and cutover matrix](synbiohub-compatibility-matrix.md)**:
  the checked complete endpoint inventory, V1-to-V2 ownership, exact versus
  semantic parity, deprecated aliases, intentional differences, unsupported
  behavior, privacy-safe usage metrics, and the no-removal decision boundary.
- **[Running SynBioHub on sbol-db](synbiohub.md)**: the triplestore
  interface classic SynBioHub reaches over HTTP, the behaviors sbol-db
  matches so it stands in for Virtuoso at runtime, and how to run
  SynBioHub's suite against it.
- **[The v2 API](api-v2.md)**: the idiomatic REST surface under
  `/api/v2`, a second presentation of the same facade the SynBioHub-compat
  surface presents, with proper HTTP verbs, JSON bodies, real pagination,
  content negotiation, a consistent error envelope, bearer auth, and the
  mapping from the v1 endpoints.
- **[Migrating a classic instance](synbiohub-migration.md)**: loading a
  deployed classic SynBioHub (RDF dump, `synbiohub.sqlite`, `uploads/`,
  and `config.local.json`) into sbol-db with `sbol-db
  migrate-synbiohub`, and verifying the result.
- **[Differential conformance suite](synbiohub-conformance.md)**: the
  parity harness that diffs sbol-db against a classic SynBioHub
  reference, how comparison works, how to run each tier, and the
  environment caveats.

## Scope

SBOL DB covers the lifecycle of biological design records: ingest, validation,
identity, discovery, contribution, publication, collaboration, exchange, and
operations. It deliberately does not expand into experiments, builds, samples,
measurements, predictive model runs, or broader DBTL workflow orchestration.
The [crate guide](crate-guide.md#scope) spells out the boundary and rationale.
