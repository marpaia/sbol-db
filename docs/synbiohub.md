# SynBioHub / Virtuoso compatibility

`sbol-db` can act as a drop-in replacement for the Virtuoso triplestore
that [SynBioHub](https://synbiohub.org) runs on. Both the classic
Node/Express SynBioHub and the newer `synbiohub3` (Java/Next.js) talk to
their triplestore over the same HTTP surface, and `sbol-db` implements
it. Point SynBioHub at an `sbol-db server` instead of Virtuoso and no
SynBioHub code changes are required.

In this mode `sbol-db` is a faithful triplestore: RDF is stored and
returned **verbatim**. SynBioHub stores SBOL2, so SBOL2 round-trips
unchanged. There is no SBOL2 to SBOL3 upgrade and no typed projection on
this path; that path is `sbol-db graph import`, which is separate.

## Endpoints

| Endpoint                          | Methods            | Auth  | Purpose |
| --------------------------------- | ------------------ | ----- | ------- |
| `/sparql`                         | `GET`, `POST`      | none  | SPARQL 1.1 read (SELECT/ASK/CONSTRUCT/DESCRIBE). Honors `default-graph-uri`. |
| `/sparql-auth`                    | `GET`, `POST`      | Basic | SPARQL 1.1 Update. The update string is read from the `query=` parameter (Virtuoso convention) or a raw `application/sparql-update` body. |
| `/sparql-graph-crud-auth/`        | `POST` `PUT` `DELETE` `GET` | Basic | Graph Store HTTP Protocol. `?graph-uri=<iri>` selects the graph. `POST` merges (appends), `PUT` replaces, `DELETE` clears, `GET` serializes. |

The graph-store path is registered with and without the trailing slash.

### Update support

`INSERT DATA`, `DELETE DATA`, `DELETE WHERE`, `DELETE/INSERT … WHERE`
(including compound `;`-separated operations), and `CLEAR`/`DROP`/`CREATE`
are supported. `LOAD` is not (SynBioHub does not use it). Writes without
an explicit `GRAPH`/`WITH` target the graph named by `default-graph-uri`.

## Authentication

The write endpoints require HTTP Basic credentials (default `dba`/`dba`,
matching a stock Virtuoso). This satisfies both SynBioHub variants:
`synbiohub3` sends Basic credentials directly, and classic SynBioHub
(`sendImmediately: false`) replies to the `401` Basic challenge with
matching credentials.

| Variable                       | Default | Meaning |
| ------------------------------ | ------- | ------- |
| `SBOL_DB_SPARQL_AUTH_USER`     | `dba`   | Basic auth username for the write endpoints. |
| `SBOL_DB_SPARQL_AUTH_PASSWORD` | `dba`   | Basic auth password. |
| `SBOL_DB_SPARQL_AUTH_DISABLED` | `false` | When `true`, the write endpoints skip auth (for trusted-network deployments behind a proxy). |

## Pointing SynBioHub at sbol-db

Set SynBioHub's triplestore endpoints to the `sbol-db server`:

```sh
# synbiohub3 (environment variables)
export SBH_SPARQL_ENDPOINT="http://sbol-db:8080/sparql"
export SBH_GRAPH_STORE_ENDPOINT="http://sbol-db:8080/sparql-graph-crud-auth/"
```

For classic SynBioHub, set the `triplestore` block in its `config.json`
(`sparqlEndpoint`, `graphStoreEndpoint`, and the authenticated SPARQL
endpoint) to the corresponding `sbol-db` URLs, keeping `username`/
`password` as `dba`/`dba` (or whatever you configured above).

## Named graphs

Graph IRIs are opaque to `sbol-db`; it stores whatever SynBioHub uses
(typically a public graph such as `https://synbiohub.org/public` and
per-user graphs like `https://synbiohub.org/user/<name>`). Graph
isolation is enforced by `default-graph-uri` on reads and the
`graph-uri` parameter on graph-store writes.

## Not yet implemented

- SPARQL federation (`SERVICE`), RDFS/OWL inference, and Virtuoso
  `bif:*` functions. SynBioHub's queries use none of these; they are
  standard SPARQL 1.1 (`CONTAINS`, `STRSTARTS`, `FILTER NOT EXISTS`,
  aggregation, property-free patterns), all of which work.
- A compound SPARQL Update evaluates each operation's `WHERE` against the
  snapshot at the start of the request, not against the uncommitted
  effects of earlier operations in the same request. SynBioHub's
  multi-operation updates are independent, so this matches its behavior.
