# Postgres schema

`sbol-db` ships nine embedded migrations under
`crates/sbol-db-postgres/migrations/`. They're applied via
`sqlx::migrate!` either through the library
(`sbol_db_postgres::run_migrations`) or the CLI (`sbol-db db migrate`).

This document is the table-by-table reference; rationale and the
"why this shape" lives in [`crate-guide.md`](crate-guide.md).

## Extensions and domains

```sql
CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE EXTENSION IF NOT EXISTS btree_gin;
CREATE EXTENSION IF NOT EXISTS pg_trgm;
```

Two domains for cheap shape checks:

- `sbol_iri text CHECK (VALUE ~ '^[a-zA-Z][a-zA-Z0-9+.-]*:.+')`. A light
  sanity check, not a full IRI parser; full validation lives in
  `sbol-rs` via `oxiri`.
- `sbol_ontology_term text` with the same shape. Carries SBO / SO /
  EDAM / GO / ChEBI / CL / NCIT term IRIs.

## `sbol_documents`

One row per imported document.

| Column                 | Type                | Notes                                                    |
| ---------------------- | ------------------- | -------------------------------------------------------- |
| `id`                   | `uuid`              | Primary key, `gen_random_uuid()`.                        |
| `document_iri`         | `sbol_iri` (unique) | Optional explicit document IRI supplied by the caller.   |
| `name`                 | `text`              | Display name.                                            |
| `description`          | `text`              | Display description.                                     |
| `serialization_format` | `text`              | One of `json|jsonld|rdfxml|turtle|trig|ntriples|nquads|genbank|fasta`. |
| `source_uri`           | `text`              | Free-form provenance (URL, filename) from the caller.    |
| `raw_payload`          | `jsonb`             | Optional lossless snapshot of the import body.           |
| `content_hash`         | `bytea`             | SHA3-256 of the original input bytes.                    |
| `created_by`           | `text`              | Free-form actor identifier.                              |
| `created_at`           | `timestamptz`       | Row-insertion timestamp.                                 |
| `updated_at`           | `timestamptz`       | Last-modification timestamp.                             |

Indexed: `raw_payload` as `gin`, document_iri unique, `id` primary key.

## `sbol_objects`

One row per top-level SBOL object, keyed by IRI. This is the
canonical KV store for "give me the object at this IRI".

| Column                | Type                   | Notes                                                              |
| --------------------- | ---------------------- | ------------------------------------------------------------------ |
| `id`                  | `uuid`                 | Internal handle for joins.                                         |
| `iri`                 | `sbol_iri` (unique)    | Object identity. Required to be an IRI per SBOL spec.              |
| `sbol_class`          | `text`                 | RDF class IRI, e.g. `http://sbols.org/v3#Component`.               |
| `persistent_identity` | `sbol_iri`             | SBOL persistent identity (the identity minus the version segment). |
| `display_id`          | `text`                 | SBOL displayId.                                                    |
| `version`             | `text`                 | SBOL version, if present.                                          |
| `name`                | `text`                 | Object name.                                                       |
| `description`         | `text`                 | Object description.                                                |
| `document_id`         | `uuid` (FK)            | References `sbol_documents.id`; `ON DELETE SET NULL`.              |
| `types`               | `sbol_ontology_term[]` | Component / object types. Gin-indexed.                             |
| `roles`               | `sbol_ontology_term[]` | Feature / participation roles. Gin-indexed.                        |
| `data`                | `jsonb`                | The per-object JSON-LD slice. Lossless, suitable for re-emit.      |
| `content_hash`        | `bytea`                | SHA3-256 of the object's canonical N-Triples.                      |
| `is_deleted`          | `boolean`              | Soft-delete flag; the live-row index excludes deleted rows.        |
| `created_at`          | `timestamptz`          | Row-insertion timestamp.                                           |
| `updated_at`          | `timestamptz`          | Last-modification timestamp.                                       |

Indexes:

- `(iri)` primary uniqueness; `(persistent_identity)` and
  `(persistent_identity, version)` for SBOL version-stack queries.
- `(display_id, document_id)` unique-by-document for displayId
  collision detection.
- gin on `types`, `roles`, `data` (jsonb_path_ops).
- trgm on `display_id` and `name` for fuzzy lookup.
- partial `(iri) WHERE is_deleted = false` for the live-object hot
  path.

## `sbol_quads`

The RDF triple/quad store. One row per imported triple.

| Column           | Type          | Notes                                                                                     |
| ---------------- | ------------- | ----------------------------------------------------------------------------------------- |
| `id`             | `bigserial`   |                                                                                           |
| `graph_iri`      | `sbol_iri`    | Named graph IRI. The import pipeline tags every quad with `graph:document:{document_id}`. |
| `subject_iri`    | `sbol_iri`    | Mutually exclusive with `subject_blank`.                                                  |
| `subject_blank`  | `text`        | Blank node identifier (without the `_:` prefix). See [Blank nodes](#blank-nodes).         |
| `predicate_iri`  | `sbol_iri`    | Required.                                                                                 |
| `object_iri`     | `sbol_iri`    | Mutually exclusive with `object_blank` and `object_literal`.                              |
| `object_blank`   | `text`        | Blank node identifier.                                                                    |
| `object_literal` | `text`        | Literal lexical form.                                                                     |
| `datatype_iri`   | `sbol_iri`    | Populated alongside `object_literal`.                                                     |
| `language`       | `text`        | BCP-47 language tag for language-tagged literals.                                         |
| `document_id`    | `uuid` (FK)   | References `sbol_documents.id`; `ON DELETE CASCADE`.                                      |
| `source`         | `text`        | Provenance tag; defaults to `'sbol'`.                                                     |
| `created_at`     | `timestamptz` |                                                                                           |

CHECK constraints enforce "exactly one" of (subject_iri,
subject_blank) and (object_iri, object_blank, object_literal).

Indexes:

- `spog` `(subject_iri, predicate_iri, object_iri, graph_iri)`.
- `posg` `(predicate_iri, object_iri, subject_iri, graph_iri)`
  partial on `object_iri IS NOT NULL`.
- `ospg` `(object_iri, subject_iri, predicate_iri, graph_iri)` partial.
- `gspo` `(graph_iri, subject_iri, predicate_iri, object_iri)`.
- trgm on `object_literal` for fuzzy string search in literal
  positions.

These cover the access shapes the SPARQL pattern scanner emits;
Postgres picks the right one based on which positions are bound.

### Blank nodes

The `sbol_iri` domain enforces `^[a-zA-Z][a-zA-Z0-9+.-]*:.+`, which
rejects `_:b0`-shaped blank-node identifiers. Real SBOL documents
routinely carry blank nodes after a parser pass (location objects,
intermediate participations, anonymous list-cells). `sbol_quads`
resolves this by carrying companion `subject_blank text` and
`object_blank text` columns with a CHECK that exactly one of the
(IRI, blank) pair is non-null per position. The repository's
pattern scanner picks the right column based on the bound term's
shape.

`sbol_objects.iri` stays IRI-only — top-level SBOL objects are
required to have IRIs per spec; the parser surfaces this as a
validation error if violated.

## Typed projections

`migrations/20260516000001_typed_sbol_projections.sql` creates seven
tables that mirror the subset of SBOL that benefits from columnar
queries. Each row corresponds 1:1 with a row in `sbol_objects` and
is keyed by `object_id`.

| Table                  | What it stores                                                                  |
| ---------------------- | ------------------------------------------------------------------------------- |
| `sbol_components`      | Component types/roles, feature/sequence/interaction/model IRI arrays.            |
| `sbol_sequences`       | `encoding_iri`, `elements`, `length_bp` (`GENERATED ALWAYS AS`), alphabet, topology. |
| `sbol_features`        | Parent component IRI, `feature_kind`, `instance_of_iri`, roles, orientation.     |
| `sbol_locations`       | Feature IRI, sequence IRI, `start_pos` / `end_pos` / `cut_pos`, location kind.   |
| `sbol_constraints`     | Parent component IRI, `restriction_iri`, subject / object IRIs.                  |
| `sbol_interactions`    | Parent component IRI, interaction types array.                                   |
| `sbol_participations`  | Interaction IRI, participant IRI, roles.                                         |

These projections are derived during import from the
`sbol::Document` typed-objects iterator and re-written
deterministically on re-import (via
`TypedProjectionRepository::upsert_all`). They are read-only from
the perspective of every other component; never write them by hand.

Indexes follow the natural query shape (parent IRI, instance-of IRI,
roles via gin, length / alphabet on sequences for range filters).

## Sequence k-mer index

`migrations/20260517000001_sequence_kmers.sql` adds the seed index for
the nucleotide substring + reverse-complement search documented in
[`sequences.md`](sequences.md).

```sql
sbol_sequence_kmers (
    sequence_object_id uuid    NOT NULL REFERENCES sbol_sequences(object_id) ON DELETE CASCADE,
    kmer               integer NOT NULL,   -- canonical 2-bit packed 8-mer
    position           integer NOT NULL,   -- 0-indexed
    strand             char(1) NOT NULL    -- '+' or '-'
);
```

The stored `kmer` is the canonical form -- `min(forward,
reverse_complement)` of the 2-bit packed nucleotide -- so a single
index probe finds both forward and RC seeds. Indexes:

- `(kmer)` for seed lookups.
- `(sequence_object_id)` to clean up on re-import.

Indexing happens inside `TypedProjectionRepository::upsert_sequence`
during import; protein and other non-nucleotide sequences are skipped.

## Ontology tables

`migrations/20260517000002_ontology.sql` carries the ontology loader
state used by `OntologyRepository::load_from_url` and
`/ontology/descendants`. See [`ontology.md`](ontology.md) for the
loader pipeline.

```sql
sbol_ontologies (prefix PK, name, source_url, version, term_count, imported_at)
sbol_ontology_terms (iri PK, prefix FK, curie, name, definition,
                     is_obsolete, synonyms text[])
sbol_ontology_term_aliases (alias_iri PK, canonical_iri FK)
sbol_ontology_closure (ancestor_iri FK, descendant_iri FK, depth)
  PRIMARY KEY (ancestor_iri, descendant_iri)
```

- `sbol_ontology_terms.name` is `gin (name gin_trgm_ops)` for fuzzy term
  lookup.
- `sbol_ontology_closure` is indexed on `(ancestor_iri)` (primary key
  prefix) and `(descendant_iri)`; the closure includes the trivial
  `(X, X, 0)` self-pair so `WHERE ancestor_iri = $1` returns the
  canonical root alongside its descendants.
- Reloads cascade through `sbol_ontologies.prefix`, so re-running
  `sbol-db ontology fetch so` atomically replaces the SO terms,
  aliases, and closure pairs without touching SBO.

## Validation

```sql
sbol_validation_runs (id, target_iri, target_document_id, validator_name,
                      validator_version, ruleset, status, summary, ...)
sbol_validation_findings (id, validation_run_id, severity, rule_id, message,
                          subject_iri, predicate_iri, object_iri, path, data)
```

One `sbol_validation_runs` row per `Document::validate()` call.
Findings are the individual rule violations / warnings. `severity` is
one of `info|warning|error|fatal`; `rule_id` carries the `sbol3-*`
identifier from `sbol-rs`'s rules table.

## Object revisions

`sbol_object_revisions` holds a per-object revision log. Each upsert
into `sbol_objects` writes a new row with the incrementing
`revision_number`, the JSON-LD slice at that revision, and a
content hash. `(object_id, revision_number)` is unique.

This is intentionally append-only history. Reverting to a prior
revision is a future concern; the table exists today so re-imports
don't silently overwrite the prior state.

## `sbol_rdf_projection_events`

```sql
sbol_rdf_projection_events (id, event_type, subject_iri, graph_iri,
                            payload, created_at, processed_at, error)
```

A projection event log. The import pipeline appends one
`document_imported` event per import; no consumer reads from this
table yet. It exists so any future async projection target — an
Oxigraph sidecar, a search index, a materialized view refresher —
can tail the log deterministically. The partial index
`WHERE processed_at IS NULL` is the natural cursor for a consumer.

## Conventions

- All primary keys are `uuid DEFAULT gen_random_uuid()` except
  `sbol_quads.id` (`bigserial`).
- All timestamps are `timestamptz DEFAULT now()`.
- Foreign keys cascade through document boundaries (`ON DELETE
  CASCADE` for quads, `ON DELETE SET NULL` for objects so individual
  objects survive a document deletion if needed).
- Soft-delete via `is_deleted boolean DEFAULT false`, with a
  partial index on the live rows.
- Free-form metadata lives in `jsonb` columns with `gin
  jsonb_path_ops` indexes; opinionated metadata gets its own
  column.

## What's intentionally not here

- **DBTL / lab / workflow tables.** See the
  [scope statement](crate-guide.md#scope).
- **Embeddings.** No `vector` extension load. The schema is
  rebuildable as-is in any managed Postgres; vector support lands
  later if needed.
- **Multi-tenancy columns.** No `organization_id`. Repositories
  accept all data via the domain types, so a tenancy layer can be
  added later without touching the access path.
- **Materialized views.** No precomputed component-summary view
  (feature / interaction counts per component) exists yet. The
  underlying joins are cheap given the indexes; the view ships when
  a caller needs a stable shape rather than the live join.
