# Migrating a classic SynBioHub instance

`sbol-db migrate-synbiohub` loads a classic (Node/Express) SynBioHub
instance into an sbol-db database in one pass. It reconstructs the
equivalent state without going through the classic app: the RDF is loaded
verbatim, accounts keep their existing password hashes, uploaded files keep
their content addresses, and the mutable config file becomes durable config.

This document covers what the command reads, how to run it against a real
instance, and how to verify the result. For the running-triplestore
compatibility surface (SynBioHub talking to sbol-db in place of Virtuoso at
runtime) see [Running SynBioHub on sbol-db](synbiohub.md).

## What a classic instance looks like

A deployed classic SynBioHub keeps its state in four places:

| Part | Classic location | What it holds |
| --- | --- | --- |
| RDF | Virtuoso triplestore | The SBOL2 corpus: a public graph plus one graph per user |
| Accounts | `synbiohub.sqlite` (`users` table) | Names, emails, affiliations, password hashes, owned graph URIs, roles |
| Uploads | `uploads/` tree | Attachment blobs, gzip-compressed and content-addressed by SHA-1 |
| Config | `config.local.json` | Instance name and URL, feature toggles, and other mutable settings |

The migration takes those same four parts. Only the RDF needs preparing: it
lives inside Virtuoso, so you dump it to a file first. The other three are
copied straight from the instance's filesystem.

## Prerequisites

- The `sbol-db` binary on the machine running the migration.
- A destination database, chosen with `--database-url` (or `DATABASE_URL`).
  The scheme selects the backend: `postgres://`, `sqlite://`, or
  `rocksdb://`. The migration applies any pending schema migrations before
  loading unless `--skip-migrations` is given.
- Read access to the classic instance's `synbiohub.sqlite`, `uploads/` tree,
  and `config.local.json`.
- A Virtuoso RDF dump exported as N-Quads (`.nq`) or TriG (`.trig`). Both
  carry graph names, which the migration needs to keep the public graph and
  each per-user graph separate. Triple-only formats (`.nt`, `.ttl`,
  `.rdf`) are accepted but land in the default graph, so they are only
  useful for a single-graph load.

### Exporting the RDF dump from Virtuoso

Virtuoso serializes its full quad store to N-Quads with `dump_nquads`. From
`isql`:

```sql
dump_nquads('dumps', 1, 1000000000, 1);
```

That writes numbered `.nq` files under Virtuoso's configured `DirsAllowed`
dump directory. Concatenate them into one file (for example `dump.nq`) and
place it wherever the migration can read it.

## Running the migration

The simplest form points `--source` at an unpacked instance directory and
`--blob-store` at the destination blob-store root:

```sh
sbol-db migrate-synbiohub \
  --database-url postgres://sbol:sbol@localhost:5432/sbol \
  --source /path/to/classic-instance \
  --blob-store /var/lib/sbol-db/blobs
```

With `--source` set, each part defaults to a conventional path beneath it:

| Part | Default path under `--source` | Override flag |
| --- | --- | --- |
| RDF dump | `dump.nq` | `--rdf` |
| Accounts | `synbiohub.sqlite` | `--sqlite` |
| Uploads | `uploads` | `--uploads` |
| Config | `config.local.json` | `--config` |

Any part can be given its own explicit path, which overrides the
`--source`-derived default. A part whose resolved path does not exist is
skipped with a warning, so a partial instance still migrates the parts it
has. When the classic pieces are scattered, drop `--source` and pass each
flag directly:

```sh
sbol-db migrate-synbiohub \
  --database-url postgres://sbol:sbol@localhost:5432/sbol \
  --rdf /dumps/public.nq \
  --sqlite /backup/synbiohub.sqlite \
  --uploads /backup/uploads \
  --config /etc/synbiohub/config.local.json \
  --blob-store /var/lib/sbol-db/blobs
```

`--blob-store` must match the blob-store root the running sbol-db server is
configured to serve from: the uploads tree is copied to `<root>/uploads`,
which is where the server looks up a blob by its SHA-1.

Useful flags:

- `--default-graph <iri>` loads any default-graph (unnamed) triples into the
  named graph you specify. Without it such triples are counted and skipped,
  since a well-formed SynBioHub dump keeps everything in named graphs.
- `--skip-migrations` assumes the destination schema is already current and
  does not apply pending migrations first.
- `--no-reindex` skips enqueuing the search-index rebuild (see below).

## What each part becomes

- **RDF** is parsed as a quad stream, grouped by graph name, and each named
  graph is written verbatim through the graph-store write path in `Replace`
  mode. The public graph and every per-user graph land byte-for-byte where
  they were, so SBOL2 round-trips unchanged and graph IRIs stay opaque.
- **Accounts** are read from the `users` table with the database opened
  read-only, so the source is never mutated. Each row becomes an account
  that keeps its legacy `sha1(salt + sha1(password))` hash and owned graph
  URI. The hash is not rehashed during migration; the first successful login
  transparently upgrades it to argon2. Admin, curator, and member roles and
  any password-reset link are carried over.
- **Uploads** are copied into `<blob-store>/uploads`, preserving the
  `<sha1[0:2]>/<sha1[2:]>.gz` shard layout byte-for-byte. The tree is already
  gzip-encoded and content-addressed, so every attachment stays retrievable
  by its content hash.
- **Config** keys are each written into the durable config store, the
  replacement for the mutable `config.local.json`.
- After loading, a `rebuild_search_index` job is enqueued so the native
  ranked text index, PageRank scores, and sequence clusters are rebuilt from
  the loaded corpus. A worker (the embedded one in `sbol-db server`, or a
  standalone `sbol-db worker`) must be running to process it.

The command prints a JSON report of what it loaded: the named graphs and
their triple counts, total triples, default-graph triples skipped, users
imported, blobs copied, config keys set, and the reindex job id.

## Post-migration verification

Point the report's numbers and a few spot checks at the loaded database. The
examples below use a server started against the migrated database.

1. **Graphs are verbatim.** Compare triple counts per graph against the
   report, and confirm the public graph reads back through SPARQL:

   ```sh
   curl -s -X POST http://localhost:8888/sparql \
     -H 'Accept: application/sparql-results+json' \
     --data-urlencode 'query=SELECT (COUNT(*) AS ?c) FROM <https://synbiohub.org/public> WHERE { ?s ?p ?o }'
   ```

2. **Accounts migrated and upgrade on login.** A migrated user logs in with
   their original password; the first login rehashes the stored credential to
   argon2 while the account's roles and owned graph URI are unchanged.

3. **Blobs are retrievable by hash.** An attachment's blob is present under
   `<blob-store>/uploads/<sha1[0:2]>/<sha1[2:]>.gz` and downloads through the
   server's attachment endpoint by its SHA-1.

4. **Config is set.** Read a known key (for example the instance name) back
   from the config store and confirm it matches the classic
   `config.local.json`.

5. **Search index rebuilt.** Once the reindex job completes, ranked text
   search, `/similar`, and sequence clustering return results over the
   migrated corpus. Check job status with `sbol-db jobs` and confirm the
   `rebuild_search_index` job reached a terminal success state.

The `migrate-synbiohub` integration test drives exactly these checks against
a synthetic mini-dump fixture, asserting verbatim graph counts, legacy-login
rehash, blob-by-hash retrieval, a config key, and the enqueued reindex job.
