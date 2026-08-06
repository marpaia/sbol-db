# RocksDB layout

This is the on-disk layout of the RocksDB backend, one of three engines that
satisfy the backend-neutral `sbol-db-storage` contract. For the contract itself
and how a backend is selected, see [storage.md](storage.md); for the Postgres
and SQLite layouts, see [schema-postgres.md](schema-postgres.md) and
[schema-sqlite.md](schema-sqlite.md).

RocksDB is an embedded, ordered key/value store. The triplestore is built
directly on it: every RDF term is stored once under a content-addressed id, and
triples live as keys in permuted indexes so that a pattern with bound leading
positions becomes a single prefix scan. The derived views (objects, graphs,
ontology, sequences, the job queue, and the SynBioHub accelerator) each get
their own column families. The source is `crates/sbol-db-rocksdb/src/`:
`db.rs` (the handle and column-family list), `codec.rs` (the term dictionary),
and `keys.rs` (the permuted-index keys).

## Database handle and options

One database holds every keyspace as a separate column family. All families
share tuned options: Snappy compression, a bloom filter (10 bits per key) with
cached index and filter blocks, and a shared 256 MiB LRU block cache. Opening
sets `create_if_missing`, `create_missing_column_families`, parallelism scaled
to the machine, and `bytes_per_sync`.

Two properties the rest of the layout leans on:

- Writes commit as one atomic `WriteBatch`. An import stages every term and
  every index key into a single batch, and `Db::write` commits them together;
  concurrent reads see the pre-write or post-write state, never a partial one.
- The insert path is get-before-put (the key being present means the triple is
  already stored). The bloom filter answers a definite "absent" without a read,
  so the common case of a new triple skips the lookup.

`Db` also holds a shared id-to-term cache (`TermDict`). A term id is a content
address and `id2term` is append-only, so a decoded id-to-term mapping is
immutable and never needs invalidation; sharing the cache across pattern scans
collapses the repeated `id2term` lookups a nested-loop join would otherwise make.

## Column families

| Column family | Role |
| --- | --- |
| `id2term` | Term dictionary: 16-byte term id to reversible term encoding. |
| `dspo`, `dpos`, `dosp` | Default-graph permuted triple indexes (3-id keys). |
| `spog`, `posg`, `ospg`, `gspo`, `gpos`, `gosp` | Named-graph permuted triple indexes (4-id keys). |
| `graph_meta`, `graph_hash` | Native document metadata and content-hash deduplication. Named-graph existence itself comes from `gspo`. |
| `verbatim_graph_meta` | Optional provenance/count overlay for a copied verbatim graph; not the graph inventory. |
| `catalog_graph`, `catalog_graph_by_iri` | Universal named-graph records and their IRI-ordered keyset index. |
| `catalog_resource_refcount`, `catalog_sequence_refcount` | Exact graph-occurrence counts per RDF resource and sequence IRI. |
| `catalog_class_resource_refcount`, `catalog_class_count` | Exact distinct-resource counts for each RDF class. |
| `objects`, `obj_by_id`, `obj_by_graph` | Derived object view. |
| `ont`, `ont_term`, `ont_term_idx`, `ont_alias`, `ont_alias_idx`, `ont_closure`, `ont_closure_idx` | Ontology, terms, aliases, and transitive closure with per-prefix secondary indexes. |
| `seq`, `seq_kmer`, `seq_kmer_by_iri` | Sequences and the k-mer seed index. |
| `job`, `job_idem`, `job_attempt`, `job_log`, `job_ready` | Async job queue. |
| `acc_meta`, `acc_meta_by_iri`, `acc_toplevel`, `acc_bytype`, `acc_member`, `acc_rootmember`, `acc_facet`, `acc_count`, `acc_dirty` | SynBioHub query accelerator and global Admin resource catalog. |
| `meta` | Schema version, exact catalog stats, generation readiness, and other counters. |

Composite secondary-index keys join their parts with a unit-separator byte
(`SEP = 0x1F`). That byte cannot occur in an IRI or a CURIE, so concatenated key
parts never collide.

## Term dictionary (`id2term`)

Every RDF term maps to a stable 16-byte id: the leading 16 bytes (128 bits) of
its SHA3 hash. Because the id is derived from the term, interning needs no
counter and no reverse lookup, and writing the same term twice is idempotent.
`id2term` maps the id back to a reversible byte encoding so a read can
materialize a full term from an index key.

The encoding is a one-byte tag followed by a payload:

| Tag | Term | Payload |
| --- | --- | --- |
| `1` | Named node | the IRI's UTF-8 bytes |
| `2` | Blank node | the identifier's UTF-8 bytes |
| `3` | Literal | `datatype_len` (u32 LE), datatype bytes, `has_language` (1 byte), `language_len` (u32 LE), language bytes, then the value bytes |

The same byte sequence is both the stored value and the input hashed to derive
the id, so distinct terms never collide — a named node and a string literal with
the same lexical form hash differently because their tags differ.

## Permuted triple indexes

A triple is four term ids: graph (G), subject (S), predicate (P), object (O).
Each index stores them concatenated in a fixed slot order, so a key is a triple
and a re-insert writes the same key — set semantics hold with no duplicate
check. Default-graph triples (no G) live in three 3-id indexes; named-graph
triples live in six 4-id indexes.

| Index | Column family | Slot order |
| --- | --- | --- |
| DSPO | `dspo` | S, P, O |
| DPOS | `dpos` | P, O, S |
| DOSP | `dosp` | O, S, P |
| SPOG | `spog` | S, P, O, G |
| POSG | `posg` | P, O, S, G |
| OSPG | `ospg` | O, S, P, G |
| GSPO | `gspo` | G, S, P, O |
| GPOS | `gpos` | G, P, O, S |
| GOSP | `gosp` | G, O, S, P |

Each id is 16 bytes; a default key is 48 bytes and a named key is 64. The
evaluator picks the index whose leading slots match the pattern's bound
positions, turning the scan into a prefix range. A pattern `(?s rdf:type ?o)`
binds P, so it scans `dpos`; `(s ?p ?o)` binds S and scans `dspo`; a pattern
that binds the graph scans one of the `g*` indexes. Keeping all nine permutations
means every triple pattern has an index whose prefix covers its bound positions.

## Universal named-graph catalog

Canonical named triples are authoritative graph contents. `catalog_graph`
provides the one graph inventory used by product and Admin APIs, whether the
graph came from a native document import, Graph Store/SPARQL writes, or a
reconciled backend conversion. `catalog_graph_by_iri` makes filtering and
keyset pagination proportional to the returned graphs. `graph_meta` and
`verbatim_graph_meta` are compatibility/provenance inputs used when rebuilding
that universal catalog; they are not alternate Admin inventories.

| Column family | Key | Value |
| --- | --- | --- |
| `graph_meta` | graph id (UUID bytes) | the graph record |
| `graph_hash` | content hash bytes | the graph id, for content-hash dedup |

## Derived object view

These rows are a rebuildable native-object cache, not the definition of which
RDF resources exist. The universal resource catalog pages the globally
IRI-ordered `acc_meta_by_iri` projection, so verbatim and native-import
resources have one representation. A database containing RDF but lacking that
projection is not served as an empty corpus: startup fails closed until the
versioned migration rebuilds it. Native imports continue to maintain the rows
below for compatibility object APIs.

| Column family | Key | Value |
| --- | --- | --- |
| `objects` | object IRI | the object record |
| `obj_by_id` | object id (UUID bytes) | the object IRI (an indirection into `objects`) |
| `obj_by_graph` | graph id bytes ++ object IRI | empty (lists a graph's objects by prefix scan on the graph id) |

## Ontology

`ont` holds one record per loaded ontology, keyed by prefix. `ont_term` and
`ont_alias` are keyed by canonical IRI and alias IRI respectively; `ont_closure`
is keyed by `ancestor ++ SEP ++ depth ++ SEP ++ descendant`, so a closure query
is a prefix scan on the ancestor IRI. Each of those three has a sibling
`*_idx` column family keyed by `prefix ++ SEP ++ ...`. The secondary indexes
exist so a reload can find and delete every row belonging to one prefix by
prefix scan, then re-insert; reloading an ontology never touches another's rows.

## Sequences

The sequence families are an optimized substring-search projection. The Admin
sequence catalog is broader: sequence identity comes from the universal RDF
type projection, and elements/encoding are read from canonical triples. Missing
`seq` rows therefore do not make the Admin corpus appear empty. For a converted
verbatim corpus, sequence search can also take the exact nucleotide IRI set
from `seq_sketch` and project `sbol:elements` from the predicate index.

`seq` holds one record per nucleotide sequence, keyed by sequence IRI. The k-mer
seed index that backs substring + reverse-complement search is stored two ways:

| Column family | Key | Purpose |
| --- | --- | --- |
| `seq_kmer` | 4-byte canonical k-mer ++ sequence IRI | seed lookup: prefix scan on the k-mer finds candidate sequences |
| `seq_kmer_by_iri` | sequence IRI ++ SEP ++ 4-byte k-mer | cleanup: prefix scan on the IRI drops a sequence's seeds on re-import |

The stored k-mer is the canonical form — `min(forward, reverse_complement)` of
the 2-bit packed 8-mer — so one prefix scan finds both strands. See
[sequences.md](sequences.md) for the search algorithm.

## Job queue

| Column family | Key | Value |
| --- | --- | --- |
| `job` | job id (UUID bytes) | the job record |
| `job_idem` | `kind ++ SEP ++ idempotency_key` | the job id, deduplicating enqueues |
| `job_attempt` | job id ++ attempt number (i32 big-endian) | the attempt record |
| `job_log` | job id ++ log id (i64 big-endian) | the log line |
| `job_ready` | `queue ++ SEP ++ priority ++ available_at ++ job id` | empty; the sorted dequeue index |

The `job_ready` key is built so RocksDB's key order is dequeue order. Priority is
two bytes in inverted offset-binary so higher priority sorts first;
`available_at` is eight bytes of offset-binary big-endian milliseconds so earlier
jobs sort first. Dequeue is a forward prefix scan on the queue that takes the
first ready job. A per-job log id counter lives in `meta` as a big-endian u64.

## SynBioHub query accelerator

Per-graph derived indexes that answer SynBioHub's fixed query templates with
point lookups and range scans. They derive from a graph's triples (the same
derivation every backend shares); graph writes refresh them in the same atomic
batch as the canonical triples. Composite keys are SEP-joined. Query indexes
start with the graph IRI; the Admin catalog's reverse projection starts with the
resource IRI so a page is one global range scan.

| Column family | Key shape | Holds |
| --- | --- | --- |
| `acc_dirty` | graph | presence means the graph's indexes are stale |
| `acc_meta` | graph ++ iri | the object's projected metadata as a JSON `MetaRecord` |
| `acc_meta_by_iri` | iri ++ graph | the same metadata, ordered for distinct global resource pagination |
| `catalog_resource_refcount` | resource iri | exact number of graph occurrences |
| `catalog_sequence_refcount` | sequence iri | exact number of graph occurrences |
| `catalog_class_resource_refcount` | resource iri ++ class iri | per-resource class occurrence count |
| `catalog_class_count` | class iri | exact number of distinct resources with the class |
| `acc_toplevel` | graph ++ displayId ++ iri | top-level objects in sort order |
| `acc_bytype` | graph ++ type ++ displayId ++ iri | objects by `rdf:type`, in sort order |
| `acc_member` | graph ++ collection ++ displayId ++ iri | collection memberships |
| `acc_rootmember` | graph ++ collection ++ displayId ++ iri | the `FILTER NOT EXISTS` anti-join: members not referenced by another member |
| `acc_facet` | graph ++ kind ++ value | distinct facet values over top-level objects |
| `acc_count` | graph ++ scope | precomputed counts, as a little-endian u64 |

## Migrations

Opening RocksDB creates missing column families, but logical projections are
versioned migrations. Schema version 2 rebuilds the reverse resource index,
graph catalog, resource/sequence/class reference counts, and exact stats from
canonical RDF plus durable compatibility metadata. The rebuild is deterministic
and resumable: its generation marker is deleted first and written only after
all projection pages and counters commit. Until that marker exists, a database
with source RDF returns an explicit unavailable error instructing the operator
to run `sbol-db db migrate`; it never renders a misleading empty corpus.

## Not yet on RocksDB

A few Postgres surfaces are not part of the RocksDB layout, and are not part of
the neutral contract either: the validation-findings audit trail and the typed
projection tables (`sbol_components` and siblings). RocksDB exposes LSM engine
introspection rather than relational tables, SQL sessions, locks, or pools.
Validation still runs during import and its status is returned in the
`ImportReport`. See the [capability matrix](storage.md#capability-matrix).
