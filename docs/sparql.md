# SPARQL

`sbol-db` exposes a read-only SPARQL 1.1 endpoint that evaluates
queries directly against `sbol_triples`. Postgres is the canonical
store; there is no sidecar triplestore, no second index to operate,
and queries always see the latest committed state.

The endpoint is implemented in the `sbol-db-sparql` crate on top of
[`spareval`](https://crates.io/crates/spareval), the standalone SPARQL
evaluator from the Oxigraph project. It is reachable via both the HTTP
server (`POST`/`GET /sparql`) and the CLI (`sbol-db query sparql`).

## Supported query forms

| Query form  | Default result format       | Other supported formats          |
| ----------- | --------------------------- | -------------------------------- |
| `SELECT`    | `application/sparql-results+json` | XML, CSV, TSV              |
| `ASK`       | `application/sparql-results+json` | XML, CSV, TSV              |
| `CONSTRUCT` | `text/turtle`               | N-Triples, JSON-LD, RDF/XML      |
| `DESCRIBE`  | `text/turtle`               | N-Triples, JSON-LD, RDF/XML      |

SPARQL Update on the public `/sparql` endpoint is rejected at parse
time with `400 Bad Request` and a `sparql_update_not_allowed` error
code. Updates are accepted only on the separate authenticated
`/sparql-auth` endpoint (see the Virtuoso-compatibility note below).

The `default-graph-uri` protocol parameter is honored: when supplied
(and the query carries no `FROM` clause of its own) it scopes the
default graph to that one named graph. Without it, the default graph is
the union of all named graphs. A query's explicit `FROM`/`FROM NAMED`
always takes precedence.

## SynBioHub / Virtuoso compatibility

`sbol-db` can stand in for Virtuoso as a SynBioHub triplestore. In
addition to read-only `/sparql`, it exposes the authenticated write
surface SynBioHub expects: `POST /sparql-auth` for SPARQL Update and
`POST|PUT|DELETE|GET /sparql-graph-crud-auth/` for the Graph Store HTTP
Protocol, both behind HTTP Basic auth. RDF posted there is stored
**verbatim** (no SBOL2 to SBOL3 upgrade). See
[`docs/synbiohub.md`](synbiohub.md).

## CLI

```sh
sbol-db query sparql <path-or-->
  [--format json|xml|csv|tsv|turtle|ntriples|jsonld|rdfxml]
  [--timeout-secs 30]
  [--max-rows 100000]
  [--max-query-size 65536]
```

`-` reads the query from stdin. The result body goes to stdout; a
truncation notice (if `--max-rows` was hit) goes to stderr.

```sh
echo 'PREFIX sbol: <http://sbols.org/v3#>
SELECT ?s WHERE { ?s a sbol:Component } LIMIT 10' \
  | sbol-db query sparql -
```

Use `sbol-db query explain <path-or-->` to parse a query without
executing it; the command prints the detected form and AST without
hitting the database.

## HTTP

Both `GET` and `POST` are supported per the
[SPARQL 1.1 Protocol](https://www.w3.org/TR/sparql11-protocol/).

```http
GET /sparql?query=<urlencoded>
GET /sparql?query=<urlencoded>&format=csv
Accept: application/sparql-results+json

POST /sparql
Content-Type: application/sparql-query
<query in body>

POST /sparql
Content-Type: application/x-www-form-urlencoded
query=<urlencoded>&format=turtle
```

Format selection priority:

1. `?format=` query parameter (or `format=` form field on POST).
2. `Accept` header. The first recognized media type wins.
3. The query form's natural default (JSON for SELECT/ASK, Turtle for
   CONSTRUCT/DESCRIBE).

Mismatches between requested format and query form (CSV for
CONSTRUCT, Turtle for SELECT) return `406 Not Acceptable` with a
`sparql_unsupported_format` error code.

## Named-graph semantics

`sbol-db` puts every imported triple in a named graph
`graph:document:{document_id}`. The default graph (the unnamed
graph in SPARQL terms) is configured as the **union of all named
graphs** so plain queries return data without requiring callers to
know about the graph policy:

```sparql
PREFIX sbol: <http://sbols.org/v3#>
SELECT ?s WHERE { ?s a sbol:Component }
```

returns every component across every imported document.

To scope to a single document, name its graph explicitly:

```sparql
PREFIX sbol: <http://sbols.org/v3#>
SELECT ?s
FROM NAMED <graph:document:550e8400-e29b-41d4-a716-446655440000>
WHERE {
  GRAPH <graph:document:550e8400-e29b-41d4-a716-446655440000> {
    ?s a sbol:Component
  }
}
```

The graph IRI embeds the `graph_id` UUID returned in the
`ImportReport` (or visible in the `sbol_graphs` table).

## Architecture

```text
SPARQL query string
  → spargebra::SparqlParser::parse_query   (rejects Updates)
  → spareval::QueryEvaluator
  → PostgresDataset (impl spareval::QueryableDataset)
  → TripleRepository::scan_pattern  (one SQL scan per triple pattern)
  → sbol_triples (Postgres)
```

The evaluator is called once per query; the dataset is called once
per triple pattern in the query plan. Each pattern lookup translates
to a single `SELECT … FROM sbol_triples WHERE …` with a dynamic
`WHERE` clause built only from the bound positions. Postgres' planner
picks the right SPOG / POSG / OSPG / GSPO index for the access shape.

### The sync/async bridge

`spareval::QueryableDataset` returns synchronous iterators; sqlx is
async. `SparqlEngine::execute` arranges the bridge with the standard
tokio pattern:

- The whole evaluation runs inside `tokio::task::spawn_blocking`.
- Each `PostgresDataset::internal_quads_for_pattern` call uses
  `tokio::runtime::Handle::current().block_on(...)` to await the
  sqlx fetch synchronously, buffers the result rows, and returns
  `Vec::into_iter()`.
- The outer `spawn_blocking` handle is wrapped in
  `tokio::time::timeout` for the per-query wall-clock cap.

This is intentionally simple — *buffer per pattern*, no streaming,
no multi-pattern fusion. The trade-off is correctness and clarity at
the cost of evaluation efficiency for very large result sets. The
right next optimization, when measurements justify it, is compiling
the BGP join down to a single SQL query per group; see
[Performance](#performance).

### Resource caps

Three knobs limit a single query:

- `timeout` (default 30s): wall-clock cap, enforced by
  `tokio::time::timeout`. Sync evaluator code cannot be preempted by
  tokio, so a query running past the deadline finishes its current
  pattern fetch before terminating. The cap is best-effort soft.
- `max_rows` (default 100 000): cap on serialized solution / triple
  rows. If hit, the response carries `X-SBOL-DB-Truncated: true`.
- `max_query_size` (default 64 KiB): rejects oversized request
  bodies with `413 Payload Too Large`.

Defaults live in `SparqlOptions::default()`; the CLI exposes
`--timeout-secs`, `--max-rows`, `--max-query-size`. The HTTP route
currently uses defaults; making them request-scoped is a future
ergonomic.

## Errors

| Error variant            | HTTP status | Trigger                                                            |
| ------------------------ | ----------: | ------------------------------------------------------------------ |
| `Parse(_)`               | 400         | Query is malformed SPARQL.                                         |
| `UpdateNotAllowed`       | 400         | Query string parses as a SPARQL Update.                            |
| `UnsupportedFormat(_)`   | 406         | Requested format is incompatible with the query form.              |
| `QueryTooLarge`          | 413         | Query body exceeds `max_query_size`.                               |
| `Timeout`                | 504         | Wall-clock cap fired.                                              |
| `Evaluation(_)`          | 500         | spareval surfaced an evaluation error.                             |
| `Serialization(_)`       | 500         | sparesults or sbol-rdf failed to write the result.                 |
| `Domain(_)`              | varies      | Underlying `DomainError` from sqlx (lifted unchanged into the API). |

CLI errors surface with the same error text; the process exits with
a non-zero status.

## Performance

`spareval-over-Postgres` is per-pattern; complex BGP joins issue one
SQL round-trip per pattern. For SBOL-scale graphs this is fine in
practice:

- Tens of millions of triples fit comfortably in Postgres with the
  indexes from `migrations/20260515000004_sbol_triples.sql` and serve
  most query shapes in single-digit milliseconds per pattern.
- The chatter doesn't dominate end-to-end latency until joins span
  many high-cardinality patterns. For those, the optimization path is
  compiling the BGP to a single SQL query (one CTE per pattern, then
  intersect) — this lives in the
  [out-of-scope list](../docs/crate-guide.md#whats-intentionally-not-here)
  pending a measured need.
- Result materialization is full-buffer; `max_rows` bounds the worst
  case.

If queries become the bottleneck, the supported escape hatches are
(in order of complexity):

1. Tighter `WHERE` clauses (the existing indexes do their job —
   `posg`/`ospg` are particularly cheap for predicate + object
   patterns).
2. Per-pattern row caps (`PATTERN_LIMIT` in `dataset.rs`, currently
   1 000 000).
3. BGP fusion in `PostgresDataset` (would override the trait's
   default per-pattern path).
4. Oxigraph sidecar built from `sbol_rdf_projection_events`.

## What's intentionally not here

- **Federated SPARQL (`SERVICE`).** Out of scope; would require a
  service handler.
- **SPARQL Update.** Out of scope. Writes go through the typed
  domain API (`SbolObjectService`). Allowing arbitrary SPARQL Update
  would bypass validation, IRI normalization, content hashing, and
  the projection event log.
- **Custom function library.** Pure SPARQL 1.1 for v1. Custom
  predicates (e.g. transitive `sbol:hasFeature*`) can be expressed
  through property paths in standard SPARQL.
- **Per-request resource caps over HTTP.** The CLI accepts `--timeout-secs`
  etc.; the HTTP route uses `SparqlOptions::default()`. Threading
  request-scoped overrides through the route is a future ergonomic.
- **Result streaming.** Results materialize into `Vec<u8>` before the
  response body is written. Streaming would require either rewriting
  the QueryableDataset iterators as `Stream`s or moving evaluation
  onto a dedicated OS thread and piping bytes through a channel.
