# Classic SynBioHub production migration

This migration preserves the classic registry's durable user-visible state:
named RDF graphs, account identities and legacy password hashes, roles, graph
ownership, sharing triples, content-addressed uploads, and instance
configuration. It is Postgres-only, manifest-gated, resumable, and fail-closed.

The old fixture loader still exists behind tests as a compatibility rehearsal,
but it is not reachable from the CLI. It was not safe for production because
it loaded an RDF file into memory, silently skipped missing inputs, copied
blobs directly into their live directory, assumed unique email addresses, had
no resume ledger, and never reconciled target content against the source.

## Production snapshot already observed

The copied production layout under `synbiohub-data` contains:

- a 4,041,211,904-byte Virtuoso database;
- a logically healthy 2,505,863,168-byte SQLite account database;
- 3,095 accounts: 16 administrators, 21 curators, and 34 members;
- 9,970 gzip upload blobs, all of which passed decompression, content SHA-1,
  path-address, and compressed SHA-256 checks;
- 94 active reset links, which the importer invalidates;
- 10,362,699 classic sessions, which are intentionally not imported;
- zero classic jobs, tasks, and external-profile rows;
- three exact duplicate-email groups and ten case-folded identity collision
  groups; and
- an HTTPS production database prefix and public graph, with login required.

The same directory also contains a misspelled 278,528-byte
`synibohub.sqlite` last modified in 2020. It has 241 accounts: 239 usernames
also occur in the selected live database and two do not. Preflight therefore
hashes it as an additional source artifact and raises
`additional_sqlite_snapshot_requires_disposition`; the cutover policy must
record why this historical snapshot is not canonical. Rotated logs and the
truncated 20-byte `backup.gz` are retained with the frozen source for audit and
recovery analysis, but are not application state imported into SBOL DB.

The target preserves exact usernames and case. Login first tries exact
username, then accepts an email only when it identifies one account. Users in
a duplicate-email group must log in with their username. No account is merged
or discarded.

Sessions, reset links, queued classic jobs, and transient tasks are operational
state rather than durable registry data. They are intentionally invalidated;
users sign in again after cutover, and incomplete work must be replayed from
the frozen source or an operator record. This is the only deliberate departure
from a literal byte-for-byte state copy.

## Architecture

```mermaid
flowchart LR
  A["Frozen classic snapshot"] --> B["Pinned Virtuoso private-copy export"]
  A --> C["Read-only SQLite private copy"]
  A --> D["Upload and config inventory"]
  B --> Q["Digest-addressed IRI normalization"]
  Q --> E["Content-addressed preflight manifest"]
  C --> E
  D --> E
  E --> F["Explicit blocker and graph policy"]
  F --> G["Postgres migration run ledger"]
  G --> H["Users and roles"]
  G --> I["Bounded RDF batches"]
  G --> J["Checksum-verified blob staging"]
  H --> K["Counts and identity reconciliation"]
  I --> L["Per-graph count and fingerprint reconciliation"]
  J --> M["Atomic upload-tree promotion"]
  K --> N["Canonical state ready"]
  L --> N
  M --> N
  N --> O["Paged derived-index rebuild"]
  O --> P["Cutover evidence and traffic switch"]
```

There are two kinds of data:

- Canonical state: accounts, configuration, RDF triples, and blobs. The loader
  will not mark this state ready until it exactly reconciles.
- Derived state: text search, PageRank, sequence sketches, and clusters. These
  are rebuilt only from reconciled canonical state and can be discarded and
  regenerated.

Every run is identified by the source bundle SHA-256 plus importer version.
Rerunning the same command resumes the same ledger. A different bundle cannot
enter a non-empty target. A Postgres advisory lock prevents two loaders from
running concurrently.

## 1. Freeze and export Virtuoso

Stop writes before taking the cutover snapshot. Keep an independently
restorable copy of every source artifact.

The exporter copies `virtuoso.db` into a temporary directory and starts the
exact inspected Virtuoso image against that private copy. Virtuoso is allowed
to checkpoint and create logs only inside the temporary directory. The source
database is mounted nowhere and cannot be mutated.

```sh
SYNBIOHUB_CONFIG=/snapshot/config.local.json \
  ./scripts/export-synbiohub-virtuoso.sh \
  /snapshot/virtuoso.db \
  /snapshot/export/production.nq \
  /path/to/large-temporary-volume
```

If the SQL DBA credential was rotated independently of SynBioHub's HTTP graph
store credential, set `VIRTUOSO_DBA_PASSWORD` from the deployment secret
manager as well. That value takes precedence over the config file and is never
printed.

The script pins
`tenforce/virtuoso@sha256:de97286328aa0babb9e06e9626321d91363ba3f260529b9083d1fa02f36ad664`,
reads the DBA password internally from the environment or classic config
without printing it,
uses `dump_nquads`, concatenates fragments in name order, atomically publishes
the output, and prints the final size and SHA-256. It refuses to overwrite an
existing export.

## 2. Normalize invalid legacy IRIs

Keep the Virtuoso export immutable. If strict N-Quads parsing identifies
legacy IRIs that are not valid absolute IRIs, review the complete inventory
and record every accepted rewrite in an exact-count, digest-addressed policy.
Then produce a new artifact and provenance report:

```sh
cargo run -p sbol-db -- normalize-synbiohub-rdf \
  --input /snapshot/export/production.nq \
  --output /snapshot/export/production.normalized.nq \
  --policy /evidence/iri-normalization-policy.json \
  --report /evidence/rdf-normalization.json
```

The normalizer scans only N-Quads IRI tokens, never literal contents. It
decodes N-Quads Unicode escapes before validation, validates every resulting
IRI as absolute, rejects unapproved invalid IRIs and target collisions, and
checks the policy's exact occurrence, replacement, IRI-role, and graph counts.
`percent_encode_spaces` preserves a malformed absolute IRI by encoding its
spaces. `map_relative_iri_to_urn` preserves an otherwise base-less relative
reference injectively under `urn:synbiohub:legacy-relative-iri:` rather than
inventing an ontology namespace. The output is strictly reparsed and must have
the same quad count as the input. Existing output and report files are never
overwritten.

## 3. Produce the preflight manifest

Preflight does not open a target database. SQLite is inspected from a private
copy including its WAL/SHM sidecars. The RDF parser and blob verifier stream
their inputs with bounded buffers.

```sh
cargo run -p sbol-db -- preflight-synbiohub \
  --source /snapshot \
  --rdf /snapshot/export/production.normalized.nq \
  --rdf-normalization-report /evidence/rdf-normalization.json \
  --config-defaults /path/to/classic-synbiohub/config.json \
  --report /evidence/synbiohub-preflight.json
```

The manifest commits to:

- byte size and SHA-256 for the raw database, raw and normalized RDF exports,
  normalization policy/report, SQLite main/WAL/SHM, and configuration files;
- a logical SHA-256 over all classic account fields in source-id order;
- every upload's expected SHA-1, content SHA-1, compressed SHA-256, compressed
  and uncompressed size, and gzip validity;
- every named graph's class, quad count, and duplicate-aware,
  order-independent fingerprint;
- account/graph, owner/account, viewer/account, and RDF/blob reconciliation;
  and
- namespace, public graph, policy flags, roles, collision summaries, and
  timestamp ranges.

The report is safe to archive: email collision values are hashed, password
hashes never appear, and nested API keys, salts, passwords, credentials,
private keys, and tokens are recursively redacted. Secret values still
contribute to the source bundle digest. At import time configuration is read
again from the hash-verified source files, not from the redacted preview.

Do not use `--allow-blockers` for a cutover manifest. That flag exists only so
an operator can save an incomplete inventory while fixing its source.

## 4. Resolve policy explicitly

No blocker can be ignored at the command line. If an exceptional source issue
is accepted, record its code and a non-empty reason in a policy file. Every
graph outside the configured public graph and user-graph namespace also needs
an explicit import disposition. A wildcard is allowed when the reviewed policy
is to preserve every such graph.

```json
{
  "waivers": {
    "reviewed_blocker_code": "Approved by migration owner in change record CHG-1234"
  },
  "other_graphs": {
    "*": "import"
  }
}
```

The production importer does not support an `exclude` disposition: a run that
drops a source graph cannot claim to be a 1:1 migration.

## 5. Rehearse on a disposable Postgres target

Use a fresh database and a blob root whose `uploads/` path is absent or empty.

```sh
cargo run -p sbol-db -- \
  --database-url postgres://sbol:REDACTED@127.0.0.1:5432/sbol_rehearsal \
  migrate-synbiohub \
  --manifest /evidence/synbiohub-preflight.json \
  --policy /evidence/synbiohub-policy.json \
  --blob-store /var/lib/sbol-db/blobs \
  --chunk-size 25000
```

The command:

1. validates manifest schema and policy;
2. rehashes every file before applying target migrations;
3. creates or resumes `sbh_migration_run` and its artifact, identity, graph,
   blob, and issue ledgers;
4. imports users from a private SQLite copy with deterministic target UUIDs,
   exact source timestamps, graph URIs, roles, and password hashes;
5. invalidates reset links and does not import sessions or API tokens;
6. writes RDF with set semantics in bounded batches and preserves every named
   graph IRI exactly;
7. checksums each copied upload into a run-specific staging tree;
8. streams every target graph in keyset pages and compares its count and
   fingerprint to the manifest;
9. reconciles identity and blob ledgers, loads transformed configuration, and
   atomically promotes the staged uploads tree; and
10. enqueues the rebuild of text, PageRank, cluster, and sequence-sketch
    indexes.

The target Graph Store no longer has the former 5,000,000-triple silent read
cap. Turtle and N-Triples responses are streamed in stable backend-keyset pages;
RDF/XML and JSON-LD remain buffered for ordinary graphs and fail explicitly
above 100,000 triples with instructions to request a streaming format.

An interrupted command is rerun unchanged. Inserts are idempotent, source user
IDs map deterministically, staged files are checksum-verified before reuse, and
the run cannot become `ready` while any canonical reconciliation fails. Running
the command again after `ready` re-verifies source files, target graph
fingerprints, identities, and the promoted upload tree.

`--no-reindex` is useful only for loader development. A cutover is not ready
until the derived-index job succeeds.

## 6. Convert a reconciled Postgres rehearsal to RocksDB

RocksDB is an embedded, single-host deployment target. Convert only from a
Postgres import whose production migration run is already `ready`, while every
API server and worker that can write to that Postgres database is stopped:

```sh
sbol-db \
  --database-url postgres://sbol:REDACTED@127.0.0.1:5432/sbol_rehearsal \
  copy-postgres-to-rocksdb \
  --destination /var/lib/sbol-db/production.rocksdb \
  --chunk-size 25000 \
  --omit-completed-job-history
```

The command refuses an active job, an unverified graph or accelerator ledger,
a dirty accelerator graph, a source without exactly one ready production run,
or a non-empty RocksDB destination belonging to another source. It does not
branch on graph kind or require optional typed-object tables: every row in
`sbol_graphs` and every canonical RDF triple is copied, then the universal
resource, graph, class, and sequence projections are rebuilt from that data.

Users, duplicate-email identity semantics, timestamps, password hashes, roles,
API-token hashes, configuration, named-graph IRIs and canonical triples, query
accelerators, PageRank, sequence clusters, sketches, LSH bands, and all
ontology/term/alias/closure rows are copied exactly. The copy also materializes
the Admin graph catalog, resource browser,
globally IRI-ordered resource index, and exact nucleotide-sequence view from the
reconciled graph ledger, accelerator metadata, and sequence-element triples.
These are compatibility projections over the unchanged SBOL 2 RDF, not a
second SBOL 2-to-3 conversion or a duplicate triple corpus. Completed job
history is operational history rather than registry state, so omitting it
requires the explicit flag shown above. Blobs and the ranked text index remain
in their durable filesystem paths and are reused at runtime.

Every page commits its source keyset checkpoint in the same RocksDB batch as
its data. Rerun the identical command after an interruption; it resumes safely.
The same stopped-server rerun also upgrades a destination completed by an older
converter: versioned projection stages replay while already-reconciled triples,
identities, and search indexes are skipped.

The target is marked `ready` only after the source is re-counted to prove that
it stayed quiescent and every destination column-family cardinality matches.

Start the first validation server without a worker so no startup reconciliation
job mutates derived state while API and restore checks run:

```sh
sbol-db \
  --database-url rocksdb:///var/lib/sbol-db/production.rocksdb \
  server --bind 127.0.0.1:8888 --no-worker
```

RocksDB admits one local process opening the database and is not a shared
multi-node backend. After validation, enable a worker only on that same host
when the deployment is ready to process new writes and maintenance jobs.

## 7. Runtime configuration

Run the server with durable paths and the original classic salts supplied from
the deployment's secret manager:

```sh
export SBOL_DB_BLOB_ROOT=/var/lib/sbol-db/blobs
export SBOL_DB_TEXT_INDEX_PATH=/var/lib/sbol-db/search/text
export SBOL_DB_PASSWORD_SALT='read from classic passwordSalt'
export SBOL_DB_SHARE_LINK_SALT='read from classic shareLinkSalt'
export SBOL_DB_SESSION_COOKIE_SECURE=true
```

`registryNamespace` is persisted during migration, so the server uses the
production HTTPS database prefix and public graph for ACLs, reads, downloads,
new user graphs, submissions, mutation targets, share URLs, and compatibility
routes. `SBOL_DB_DATABASE_PREFIX` and `SBOL_DB_PUBLIC_GRAPH` remain explicit
runtime overrides.

The original password salt is required until every legacy SHA-1 credential has
successfully logged in and transparently upgraded to Argon2 or has been reset.
The share-link salt preserves already-issued private-object share URLs.

## 8. Cutover gates

Record all evidence against the exact manifest/run pair. Do not switch traffic
until all gates pass:

- classic writes remained frozen from snapshot through decision;
- raw source and Postgres/blob backups restore independently;
- the migration run is `ready`, with all artifacts, identities, graphs, and
  blobs verified and no unwaived blocker;
- source and target totals match by graph class and for every individual graph;
- all public referenced blobs are present and retrievable;
- sampled administrators, curators, members, duplicate-email users, and
  ordinary users log in with existing usernames/passwords and keep their roles;
- sampled users see all owned collections across multiple workspace pages and
  see explicitly shared private objects;
- public anonymous search, object details, download formats, attachments, and
  collection membership work under the production HTTPS identities;
- a complete streamed N-Triples export of the target public graph matches the
  manifest fingerprint and count (no response-size truncation);
- the derived-index job succeeded and search, similar-object, and sequence
  results cover both public and private authorized scopes; and
- logs and archived evidence contain no source secrets.

Start with an operator/admin validation session, then a controlled user cohort,
then normal traffic. Keep the frozen classic service available but read-only
during the rollback window.

## Rollback

Before accepting SBOL DB writes, rollback is a traffic switch to the frozen
classic deployment. The failed/rehearsal Postgres database and blob root are
disposable; do not mutate the source snapshot to clean up a target.

After accepting SBOL DB writes, rollback is no longer a simple traffic switch.
Freeze SBOL DB, export and reconcile post-cutover writes, and make an explicit
data-loss decision before returning to classic. This tooling does not claim to
reverse-migrate SBOL DB state into classic SynBioHub.
