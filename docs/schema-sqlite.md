# SQLite schema

This is the on-disk layout of the SQLite backend, one of three engines that
satisfy the backend-neutral `sbol-db-storage` contract. For the contract itself
and how a backend is selected, see [storage.md](storage.md); for the Postgres
and RocksDB layouts, see [schema-postgres.md](schema-postgres.md) and
[schema-rocksdb.md](schema-rocksdb.md).

The SQLite backend mirrors the Postgres model with portable types. UUIDs and
timestamps are `TEXT` (timestamps are sortable RFC3339, all UTC), ontology-term
and type/role arrays are JSON in `TEXT`, and content hashes are `BLOB`. The five
migrations under `crates/sbol-db-sqlite/migrations/` run on connect via
`connect_and_migrate`, or through the CLI's `sbol-db db migrate`.

## `sbol_graphs`, `sbol_triples`, `sbol_objects`

`20260101000001_graphs_triples_objects.sql` creates the triplestore and the
derived object view.

### `sbol_graphs`

One row per named graph. A graph owns its triples: deleting the graph row
cascades them away. `kind` records normalization (`sbol3` for an imported
document, `verbatim` for raw RDF stored as written).

| Column | Type | Notes |
| --- | --- | --- |
| `iri` | `TEXT` | Primary key. The named graph IRI; an import uses `graph:document:{id}`. |
| `id` | `TEXT` | Unique. UUID surrogate key; the join target for `sbol_objects.graph_id`. |
| `kind` | `TEXT` | `sbol3` or `verbatim`. |
| `document_iri` | `TEXT` | Optional explicit document IRI from an imported document. |
| `name` | `TEXT` | Display name. |
| `description` | `TEXT` | Display description. |
| `serialization_format` | `TEXT` | The import's source format. |
| `source_uri` | `TEXT` | Free-form provenance (URL, filename). |
| `content_hash` | `BLOB` | SHA3-256 of the original input bytes. |
| `created_by` | `TEXT` | Free-form actor identifier. |
| `created_at` | `TEXT` | Insertion timestamp. |
| `updated_at` | `TEXT` | Last-modification timestamp. |

Index: `sbol_graphs_kind_hash` on `(kind, content_hash)` for content-hash
dedup lookups.

### `sbol_triples`

One row per RDF triple. A named-graph triple references its owning graph through
`graph_iri` with `ON DELETE CASCADE`; a default-graph triple carries a `NULL`
`graph_iri` (no owner, no cascade).

| Column | Type | Notes |
| --- | --- | --- |
| `id` | `INTEGER` | Primary key, autoincrement. |
| `graph_iri` | `TEXT` | FK to `sbol_graphs(iri)`, `ON DELETE CASCADE`. `NULL` for the default graph. |
| `subject_iri` | `TEXT` | Mutually exclusive with `subject_blank`. |
| `subject_blank` | `TEXT` | Blank-node identifier. |
| `predicate_iri` | `TEXT` | Required. |
| `object_iri` | `TEXT` | Mutually exclusive with `object_blank` and `object_literal`. |
| `object_blank` | `TEXT` | Blank-node identifier. |
| `object_literal` | `TEXT` | Literal lexical form. |
| `datatype_iri` | `TEXT` | Datatype IRI alongside `object_literal`. |
| `language` | `TEXT` | Language tag for language-tagged literals. |
| `source` | `TEXT` | Provenance tag (`sbol`, `graph-store`, `sparql-update`). |
| `triple_key` | `BLOB` | Not null, unique. The set-identity hash. |

Indexes:

- `sbol_triples_spog` on `(subject_iri, predicate_iri, object_iri, graph_iri)`.
- `sbol_triples_posg` on `(predicate_iri, object_iri, subject_iri, graph_iri)`.
- `sbol_triples_graph` on `(graph_iri)`.

The `triple_key` is how SQLite enforces set semantics: a graph is a set of
triples, so re-writing an already-present triple is a no-op. Postgres uses a
generated `md5` column for this, which SQLite has no equivalent for; instead the
key is a hash computed in Rust over all RDF positions. The hash also sidesteps
SQLite's index-size limit on large sequence literals in `object_literal`.

### `sbol_objects`

One row per top-level SBOL object, keyed by IRI. The derived view for "give me
the object at this IRI". `graph_id` references the owning graph with
`ON DELETE SET NULL`, so objects survive a graph deletion. Types and roles are
JSON arrays; the per-object JSON-LD slice is `data`.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | `TEXT` | Primary key (UUID). |
| `iri` | `TEXT` | Unique. Object identity. |
| `sbol_class` | `TEXT` | RDF class IRI, e.g. `http://sbols.org/v3#Component`. |
| `display_id` | `TEXT` | SBOL displayId. |
| `name` | `TEXT` | Object name. |
| `description` | `TEXT` | Object description. |
| `graph_id` | `TEXT` | FK to `sbol_graphs(id)`, `ON DELETE SET NULL`. |
| `types` | `TEXT` | JSON array, default `[]`. |
| `roles` | `TEXT` | JSON array, default `[]`. |
| `data` | `TEXT` | JSON, default `{}`. The lossless per-object JSON-LD slice. |
| `content_hash` | `BLOB` | SHA3-256 of the object's canonical N-Triples. |
| `is_deleted` | `INTEGER` | Soft-delete flag, default 0. |
| `created_at` | `TEXT` | Insertion timestamp. |
| `updated_at` | `TEXT` | Last-modification timestamp. |

Indexes: `sbol_objects_graph` on `(graph_id)`, `sbol_objects_class` on
`(sbol_class)`.

## Job queue

`20260101000002_jobs.sql` creates the async job queue, its per-attempt audit
trail, and its logs. Lease-based dequeue is a single atomic
`UPDATE ... RETURNING` under SQLite's write lock; with the writer serialized
there is no need for the `SKIP LOCKED` the Postgres backend uses.

### `sbol_jobs`

| Column | Type | Notes |
| --- | --- | --- |
| `id` | `TEXT` | Primary key (UUID). |
| `kind` | `TEXT` | Handler kind. |
| `status` | `TEXT` | Default `queued`. |
| `priority` | `INTEGER` | Default 0. |
| `queue` | `TEXT` | Default `default`. |
| `payload` | `TEXT` | JSON, default `{}`. |
| `result` | `TEXT` | JSON result on success. |
| `error` | `TEXT` | Error text on failure. |
| `idempotency_key` | `TEXT` | Deduplicates enqueues. |
| `attempts` | `INTEGER` | Default 0. |
| `max_attempts` | `INTEGER` | Default 5. |
| `available_at` | `TEXT` | Earliest dequeue time. |
| `leased_by` | `TEXT` | Worker id holding the lease. |
| `lease_expires_at` | `TEXT` | Lease expiry; a reaper reclaims past this. |
| `parent_job_id` | `TEXT` | Parent job, for fan-out. |
| `correlation_id` | `TEXT` | Caller-supplied correlation. |
| `created_at` | `TEXT` | Enqueue time. |
| `started_at` | `TEXT` | First-attempt start. |
| `finished_at` | `TEXT` | Terminal-state time. |

Indexes: `sbol_jobs_dequeue` on `(queue, status, priority, available_at)` (the
dequeue scan), `sbol_jobs_status` on `(status)`, `sbol_jobs_correlation` on
`(correlation_id)`.

### `sbol_job_attempts`

One row per attempt. `(job_id, attempt_no)` is unique; `job_id` cascades on
delete.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | `INTEGER` | Primary key, autoincrement. |
| `job_id` | `TEXT` | FK to `sbol_jobs(id)`, `ON DELETE CASCADE`. |
| `attempt_no` | `INTEGER` | Attempt number. |
| `worker_id` | `TEXT` | Worker that ran the attempt. |
| `started_at` | `TEXT` | Attempt start. |
| `finished_at` | `TEXT` | Attempt end. |
| `status` | `TEXT` | Attempt outcome. |
| `error` | `TEXT` | Error text, if any. |

### `sbol_job_logs`

Structured per-job log lines. `job_id` cascades on delete.

| Column | Type | Notes |
| --- | --- | --- |
| `id` | `INTEGER` | Primary key, autoincrement. |
| `job_id` | `TEXT` | FK to `sbol_jobs(id)`, `ON DELETE CASCADE`. |
| `attempt_no` | `INTEGER` | Owning attempt, if any. |
| `level` | `TEXT` | Log level. |
| `message` | `TEXT` | Log message. |
| `fields` | `TEXT` | JSON structured fields, default `{}`. |
| `created_at` | `TEXT` | Log timestamp. |

Index: `sbol_job_logs_job` on `(job_id, id)` for ordered log reads.

## Ontology

`20260101000003_ontology.sql` stores an ontology, its terms, alias IRIs, and the
precomputed transitive `is_a` closure. Terms, aliases, and closure rows all
carry `prefix` with `ON DELETE CASCADE` back to the ontology, so reloading is a
single `DELETE` of the ontology row followed by fresh inserts.

```
sbol_ontologies (prefix PK, name, source_url, version, term_count, imported_at)
sbol_ontology_terms (iri PK, prefix FK, curie, name, definition,
                     is_obsolete, synonyms JSON)
sbol_ontology_term_aliases (alias_iri PK, canonical_iri, prefix FK)
sbol_ontology_closure (ancestor_iri, descendant_iri, depth, prefix FK,
                       PRIMARY KEY (ancestor_iri, descendant_iri))
```

Indexes: `sbol_ontology_terms_prefix` on `(prefix, curie)`,
`sbol_ontology_closure_ancestor` on `(ancestor_iri)` for closure lookups.

## Sequences

`20260101000004_sequences.sql` stores nucleotide sequences and the canonical
8-mer seed index that backs substring + reverse-complement search. Both tables
are keyed by sequence IRI, which is what the search joins on. (The Postgres
backend keys its k-mer table by object id; SQLite keys both by IRI.)

```
sbol_sequences (iri PK, encoding_iri, elements, alphabet, content_hash)
sbol_sequence_kmers (sequence_iri, kmer INTEGER, position INTEGER, strand TEXT)
```

The stored `kmer` is the canonical form — `min(forward, reverse_complement)` of
the 2-bit packed 8-mer — so one index probe finds both strands. Indexes:
`sbol_sequence_kmers_kmer` on `(kmer)` for seed lookups,
`sbol_sequence_kmers_iri` on `(sequence_iri)` for cleanup on re-import. See
[sequences.md](sequences.md) for the search algorithm.

## SynBioHub query accelerator

`20260101000005_accelerator.sql` adds per-graph derived indexes that answer
SynBioHub's fixed query templates with point lookups and range scans instead of
graph-pattern evaluation. The indexes derive from a graph's triples (the same
derivation every backend shares). A write marks the graph dirty in
`accel_dirty`; the next read that needs the indexes rebuilds them in one pass.

| Table | Key | What it holds |
| --- | --- | --- |
| `accel_dirty` | `graph_iri` | Presence means the graph's indexes are stale. |
| `accel_object` | `(graph_iri, iri)` | One row per object: `sort_key`, `top_level` flag, and the projected metadata as a JSON `MetaRecord`. |
| `accel_type` | `(graph_iri, type_iri, iri)` | One row per `(object, rdf:type)`, for `ByType` enumeration and counts. |
| `accel_member` | `(graph_iri, collection_iri, member_iri)` | One row per collection membership; `is_root` is the precomputed `FILTER NOT EXISTS` anti-join. |
| `accel_facet` | `(graph_iri, kind, value)` | Distinct facet values over top-level objects: kind 1 = `rdf:type`, 2 = `sbol2:role`, 3 = `dc:creator`. |

Supporting indexes: `accel_object_toplevel_idx` on
`(graph_iri, sort_key, iri) WHERE top_level`, `accel_type_scan_idx` on
`(graph_iri, type_iri, sort_key, iri)`, `accel_member_scan_idx` on
`(graph_iri, collection_iri, sort_key, member_iri)`.

## Not yet on SQLite

A few Postgres surfaces are not part of the SQLite layout, and are not part of
the neutral contract either: the validation-findings audit trail, the typed
projection tables (`sbol_components` and siblings), and engine introspection.
Validation still runs during import and its status is returned in the
`ImportReport`; SQLite reports a validation-run count of zero because it keeps no
findings table. See the [capability matrix](storage.md#capability-matrix).
