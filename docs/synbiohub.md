# Running SynBioHub on sbol-db

SynBioHub stores SBOL data in an RDF triplestore and reaches it only over
HTTP. It ships against Virtuoso, but nothing in the application binds to
Virtuoso itself: every read, write, and bulk load goes through a small set
of HTTP endpoints. sbol-db answers that same surface, so SynBioHub can run
against sbol-db in place of Virtuoso without changing how it stores or
queries data.

This document describes that interface, the behaviors sbol-db matches so
SynBioHub treats it like its existing triplestore, the compatibility
issues the swap exposes, and how to run SynBioHub's test suite against
sbol-db. It covers classic (Node/Express) SynBioHub; `synbiohub3` talks to
the same surface.

## The interface SynBioHub expects

SynBioHub's configuration has a `triplestore` block with two endpoint URLs
and a username/password. It derives a third endpoint from `sparqlEndpoint`
by appending `-auth`. So three endpoints carry all triplestore traffic:

| Endpoint | Methods | Auth | Purpose |
| --- | --- | --- | --- |
| `/sparql` | GET, POST | none | SPARQL 1.1 reads (SELECT, ASK, CONSTRUCT, DESCRIBE) |
| `/sparql-auth` | GET, POST | Basic | SPARQL 1.1 Update |
| `/sparql-graph-crud-auth/` | GET, PUT, POST, DELETE | Basic | Graph Store Protocol (bulk load) |

Reads carry a `default-graph-uri` parameter that scopes the query to one
named graph. Updates arrive with the update string in the `query=`
parameter (a Virtuoso convention, not the standard `update=`) or as a form
body; a raw `application/sparql-update` body also works. Bulk loads use the
Graph Store Protocol with `graph-uri=<iri>`: POST appends, PUT replaces,
DELETE clears, GET serializes.

The graph-store path is registered with and without the trailing slash.

## Storage model

In this mode sbol-db is a faithful triplestore. RDF that SynBioHub writes
is stored and returned without transformation. SynBioHub stores SBOL2, so
SBOL2 round-trips unchanged. There is no SBOL2-to-SBOL3 upgrade and no
typed projection on this path. Those belong to `sbol-db graph import`, a
separate ingestion path with its own semantics.

Graph IRIs are opaque to sbol-db. SynBioHub uses a public graph (typically
`https://synbiohub.org/public`) and a graph per user
(`https://synbiohub.org/user/<name>`); sbol-db stores whatever it is sent.
Without an explicit `FROM` clause a query reads the union of all named
graphs. `default-graph-uri` on a read scopes it to a single graph, and a
query's `FROM`/`FROM NAMED` clauses define its dataset.

## Matching Virtuoso's HTTP behavior

Two behaviors that SynBioHub depends on are not pinned down by the SPARQL
or Graph Store specifications. sbol-db matches Virtuoso's choices so
SynBioHub's existing code works unchanged.

Update responses use the `callret-0` convention. SynBioHub parses every
update response as SPARQL-results-JSON and reads a binding named
`callret-0`, which is how Virtuoso reports update status. Its delete loop
re-issues a DELETE until that status string contains the text
`nothing to do`. sbol-db returns a SPARQL-results-JSON body with one
`callret-0` literal: a status that ends in `done` when triples change, and
exactly `nothing to do` when an update is a no-op, so the loop terminates.
A response shaped any other way makes SynBioHub throw while reading
`results.head.vars`, which fails every update path, not just deletes.

`Accept: text/plain` returns N-Triples. SynBioHub's recursive object fetch
requests CONSTRUCT results as `text/plain`, concatenates the paged
responses, and parses the result as N-Triples. text/plain has no
registered RDF media type, but the line-based plain-text serialization is
N-Triples, which is also what Virtuoso returns. sbol-db serves N-Triples
for `text/plain`. Returning Turtle instead breaks the concatenation: the
chunks no longer parse, and the document SynBioHub rebuilds loses every
triple past the first chunk. That path drives makePublic, so the symptom
is a published collection that keeps its root object and drops all of its
members.

## Compatibility issues the strict parser exposes

sbol-db uses a spec-compliant SPARQL parser. Several queries SynBioHub
sends are valid Virtuoso extensions but not valid SPARQL 1.1. Virtuoso
accepts them; a strict store rejects them, and because SynBioHub's HTTP
layer turns a query error into an empty result, the rejection often
surfaces somewhere downstream rather than at the query. Each one has a
corresponding fix in SynBioHub, since the query is wrong rather than the
store:

- A dataset clause inside a subquery. The facet count query put its
  `FROM` clauses on a nested `SELECT`. Dataset clauses are only valid on a
  top-level query, so the count failed and search returned zero results.
- `FROM <null>`. The recursive fetch for a purely public object
  interpolated a null graph into the dataset clause, producing
  `FROM <null>`. `<null>` is a relative, meaningless IRI, and the parse
  error propagated until SynBioHub read a field off the empty result and
  crashed the request.
- A ground insert without `DATA`. The attachment writers used
  `INSERT { ...ground triples... }` with no `DATA` keyword and no `WHERE`
  clause. A modify operation needs a `WHERE`; a ground insert needs
  `INSERT DATA`.
- An empty literal. makePublic wrote published objects with an empty
  `dc:creator`. Virtuoso silently drops empty-string literals, so the bad
  triple was invisible against it; a faithful store keeps and returns it.

The pattern is the same each time: Virtuoso's leniency hid a latent bug,
and the strict store made it visible.

## When sbol-db output differs from the recorded fixtures

Some pages render triplestore contents directly, so a more correct store
produces different output than Virtuoso did. The admin graphs page lists
every named graph. Against Virtuoso it includes Virtuoso's internal system
graphs (the virtrdf, ldp, DAV, sparql, and owl graphs) next to the SBOL
data graphs. sbol-db holds only the data graphs and lists exactly those.

Before accepting such a difference as "sbol-db is cleaner," confirm the
store is omitting noise rather than dropping data. One advanced-search
facet looked like sbol-db removing a duplicate collection; it was in fact
sbol-db restoring a collection that the makePublic data loss had dropped,
and the original fixture was right. Re-baseline a fixture only after
checking that every item the old one asserted is either present or
genuinely should not be.

## Authentication

The write endpoints accept HTTP Basic credentials, default `dba`/`dba` to
match a stock Virtuoso. Classic SynBioHub authenticates with
`sendImmediately: false`: it sends the first request without credentials,
expects a 401, and retries with auth. That challenge interacts badly with
the large chunked bodies SynBioHub streams to the graph-store endpoint,
because the connection can close on the 401 before the body finishes
writing, which the Node client reports as a write `EPIPE`. On a trusted
network, disabling write auth avoids the challenge entirely.

| Variable | Default | Meaning |
| --- | --- | --- |
| `SBOL_DB_SPARQL_AUTH_USER` | `dba` | Basic auth username for the write endpoints |
| `SBOL_DB_SPARQL_AUTH_PASSWORD` | `dba` | Basic auth password |
| `SBOL_DB_SPARQL_AUTH_DISABLED` | `false` | When `true`, the write endpoints skip auth |

## Pointing SynBioHub at sbol-db

Set SynBioHub's `triplestore` block to the `sbol-db server` URLs, keeping
`username`/`password` as `dba`/`dba` (or whatever you set above):

```json
{
  "triplestore": {
    "sparqlEndpoint": "http://sbol-db:8890/sparql",
    "graphStoreEndpoint": "http://sbol-db:8890/sparql-graph-crud-auth/",
    "username": "dba",
    "password": "dba"
  }
}
```

SynBioHub derives the update endpoint as `sparqlEndpoint` + `-auth`, so
`http://sbol-db:8890/sparql-auth` must resolve to sbol-db as well. The
port is whatever `sbol-db server --bind` listens on; the host is wherever
sbol-db runs.

## Running SynBioHub's test suite against sbol-db

The harness lives in the SynBioHub repository under `tests/sboldb/`. It
runs SynBioHub's Python test suite against sbol-db with the same recorded
fixtures that define the Virtuoso baseline, so the two backends are held
to the same output.

- `docker-compose.yml` runs SynBioHub, sbol-db, and Postgres. The sbol-db
  service answers at the `virtuoso` network alias on port 8890, so
  SynBioHub's configuration is identical to the Virtuoso baseline and
  pages that display the endpoint URL match the recorded fixtures.
- `config.local.json` points SynBioHub's triplestore block at sbol-db and
  is seeded into the container as its initial config.
- `test-sboldb.sh` brings the stack up, waits for SynBioHub to report
  healthy, and runs the suite. With `--persist` it then restarts the stack
  with volumes intact and runs `test_docker_persist.py` to confirm data
  survives a restart (sbol-db data lives in Postgres, SynBioHub state in
  the `sbh` volume).
- `run-sboltestrunner.sh` builds the SBHEmulator and SBOLTestRunner jars
  in a Java 8 + Maven container and runs the SBOL2 round-trip conformance
  suite against the running stack.

The same synbiohub image serves both backends. Triplestore endpoints come
from config rather than being baked into the image, so one image runs
against either store.

All three suites pass against sbol-db. The Python suite (`test_suite.py`)
covers setup, registration, submission, collection create and delete,
makePublic, search and faceted search, SBOL/GFF/OMEX download, edit,
attachment, and the admin pages. The persistence phase confirms the
submitted data survives a container restart. The Java SBOLTestRunner
round-trips all 189 SBOLTestSuite files (submit, retrieve, compare).

## Not implemented

- SPARQL federation (`SERVICE`), RDFS/OWL inference, and Virtuoso `bif:*`
  functions. SynBioHub's queries use none of these; its searches are
  standard SPARQL 1.1 (`CONTAINS`, `STRSTARTS`, `FILTER NOT EXISTS`,
  aggregation), which work.
- A compound SPARQL Update evaluates each operation's `WHERE` against the
  state at the start of the request, not against the uncommitted effects
  of earlier operations in the same request. SynBioHub's multi-operation
  updates are independent, so this matches its behavior.
