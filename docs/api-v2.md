# The v2 API

The v2 API is SBOL DB's native REST surface, mounted under `/api/v2`. Every v2
handler calls the same application facade verbs the SynBioHub-compatible v1
adapter calls; the two surfaces read and write one dataset, one identity model,
and one ACL scope. v2 holds no business logic of its own. It differs from v1
only in wire shape.

v1 exists to let an unmodified SynBioHub client talk to sbol-db, so it inherits
SynBioHub's wire quirks: `GET` requests that mutate, the `/search/key=value`
path grammar, `multipart/form-data` for structured input, an `X-authorization`
header, and per-shape response bodies. v2 fixes those:

- Proper HTTP verbs. `GET` only reads, `POST` creates, `PATCH` edits, `DELETE`
  deletes. No `GET` mutates.
- JSON request bodies, not the `/search/key=value` path grammar or form posts.
- Real pagination: `limit`/`offset` with a `total` in the response.
- Content negotiation through `Accept`.
- One consistent JSON error envelope.
- Bearer-token authentication for API clients and an HttpOnly same-origin
  browser session for the embedded portal.

Both surfaces are ACL-scoped and identity-aware through the same graph scope
and account. An object written through one surface is visible through the
other, and the same query returns the same object set on both. The conformance
suite proves this parity on every backend.

## Authentication and browser sessions

API clients authenticate with a bearer token:

```
Authorization: Bearer <token>
```

The token is the same login token v1 reads from its `X-authorization` header;
both resolve through the one identity store. Obtain one from the v1 `POST
/login` route.

The embedded portal creates an HttpOnly session with `POST /api/v2/session`.
Its cookie carries the same opaque token but never exposes it to frontend
JavaScript. `GET /api/v2/session` returns a deliberately safe account
projection, and `DELETE /api/v2/session` revokes the token and expires the
cookie. The cookie is `SameSite=Lax`; set `SBOL_DB_SESSION_COOKIE_SECURE=true`
for every HTTPS deployment so it also carries `Secure`.

When a request presents both transports, an explicit bearer header wins. Even
a malformed Authorization header does not silently fall through to ambient
cookie authority. Unsafe cookie-authenticated requests require a same-origin
`Origin` (or equivalent browser fetch metadata), which protects mutations from
CSRF. The server's permissive legacy CORS layer does not enable cross-origin
credentialed requests.

Authentication is tolerant, matching the visibility a v1 client gets. A
missing, malformed, or unrecognized token is treated as anonymous rather than
rejected on resource routes, and an anonymous caller is scoped to the public
graph. A read outside the caller's scope is a non-disclosing `404`. A mutating
verb with no identity is `403`; invalid credentials sent to the session-create
endpoint are `401`. If the instance policy sets `require_login`, anonymous
resource routes also return `401`; the version, instance bootstrap, session,
OpenAPI, and docs endpoints remain public so a client can discover the policy
and sign in.

## Content negotiation

Read handlers choose their representation from the request's `Accept` header,
parsed in descending q-value order with the first supported type winning:

| `Accept` | Representation |
| --- | --- |
| absent, `*/*`, `application/*`, `application/json` | idiomatic JSON |
| `text/turtle`, `application/x-turtle` | RDF closure as Turtle |
| `application/rdf+xml` | RDF closure as RDF/XML |
| `application/ld+json` | RDF closure as JSON-LD |
| `application/n-triples` | RDF closure as N-Triples |

An `Accept` header that names only unsupported media types is `406 Not
Acceptable`.

The JSON representation of an object is its derived metadata record. The RDF
representations are the object's stored triples, so they serve verbatim
submissions (which carry no derived record) as well as imported objects.

## Pagination

List and search responses carry a window of items plus the total number of
in-scope matches and the applied paging:

```json
{
  "items": [ ... ],
  "total": 128,
  "offset": 0,
  "limit": 50
}
```

`limit` defaults to 50 and is clamped to `[1, 1000]`. `offset` defaults to 0.
`total` is the full count under the caller's scope, independent of the page
window, so a client can compute the number of pages.

## Error envelope

Every error is one JSON shape:

```json
{
  "error": {
    "code": "not_found",
    "message": "object http://example.org/cd/1",
    "status": 404
  }
}
```

`status` is the HTTP status, repeated in the body. `code` is a stable machine
string. `message` is human-readable. The status and code come from the same
mapping the native API uses, so the two surfaces never diverge on the meaning
of an error.

Common codes: `invalid_input` (400), `forbidden` (403), `not_found` (404),
`not_acceptable` (406), `timeout` (504).

## Resources

Base path: `/api/v2`.

### `GET /api/v2`

The version and health probe. Public; no token required.

```json
{ "name": "sbol-db", "api": "v2", "version": "0.1.2" }
```

### Instance bootstrap

**`GET /api/v2/instance`** is the public bootstrap contract for the SBOL DB
application. It returns only deployment context, public access policy, setup
state, and endpoint capabilities; it cannot return legacy visual-theme values,
mail credentials, plugin settings, or another admin-only configuration section.
`setup_required` is derived from whether an administrator exists, rather than
trusting a mutable theme flag.

```json
{
  "name": "SBOL DB",
  "instance_url": "https://parts.example.org",
  "uri_prefix": "https://parts.example.org/",
  "front_page_text": "A curated registry",
  "setup_required": false,
  "policies": {
    "allow_public_signup": true,
    "require_login": false
  },
  "capabilities": {
    "browser_sessions": true,
    "legacy_api": true,
    "structured_search": true,
    "sequence_search": true,
    "data_lab": true,
    "sql_console": true
  }
}
```

`front_page_text` is instance-authored content. A client must treat it as
untrusted input and sanitize it if it chooses to render markup.

### Session

**`POST /api/v2/session`** accepts JSON with an `identifier` (username or
email) and `password`, establishes the browser cookie, and returns the safe
session projection. The classic names `email` and `username` are accepted as
aliases for `identifier`.

```json
{ "identifier": "alice@example.org", "password": "..." }
```

**`GET /api/v2/session`** returns `200` for both states, allowing the portal to
bootstrap without using exceptions for ordinary logged-out state:

```json
{ "authenticated": false, "user": null }
```

An authenticated `user` includes the account id, username, display name,
email, affiliation, owned graph URI, role flags, and timestamps. It never
includes a password hash, reset link, or plaintext token.

**`DELETE /api/v2/session`** revokes the selected bearer or cookie token,
expires the cookie, and returns `204`. It is idempotent for anonymous, stale,
or already-revoked credentials.

### Objects

An object's whole lifecycle lives under one path, `/api/v2/objects/{iri}`. The
IRI is a single percent-encoded path segment, not the v1 path grammar. The
verb decides the action.

**`GET /api/v2/objects`** lists objects with pagination. `?q=` narrows by
free text, `?type=` restricts to one rdf:type (a full IRI), `?limit=`/`?offset=`
page. Returns the paginated envelope of search hits. This is the same ranked,
ACL-scoped query as `/search`.

**`GET /api/v2/objects/{iri}`** reads one object under the caller's scope. An
out-of-scope or unknown object is a non-disclosing `404`. The representation is
chosen by:

- `?format=`: an explicit download of the object's closure. One of `sbol`,
  `sbolnr` (non-recursive), `gb` (GenBank), `fasta`, `gff` (GFF3), `omex`.
- `?version=`: `sbol2` or `sbol3` (default) for the RDF-bearing formats; the
  sequence formats ignore it.
- otherwise `Accept` selects idiomatic JSON metadata or the object's RDF
  closure.

**`PATCH /api/v2/objects/{iri}`** edits the object's mutable fields in place.
JSON body; every field is optional, and only present fields are applied:

```json
{
  "name": "New title",
  "description": "New description",
  "mutable_description": "...",
  "mutable_notes": "...",
  "mutable_source": "...",
  "citations": ["12345678"]
}
```

Identity-gated through the facade: an anonymous caller is `403`; a non-owner
(or a non-admin editing a public object) is rejected. Returns the edited object
record, or `{"iri": "..."}` when the target is a verbatim submission with no
derived record.

**`DELETE /api/v2/objects/{iri}`** removes a top-level object and everything
whose `sbh:topLevel` names it. Returns `204 No Content`. A missing object is
`404`; an unauthorized caller is `403`.

**`POST /api/v2/objects/{iri}/publish`** publishes a private object to the
public graph under freshly minted public URIs (classic makePublic). JSON body:

```json
{
  "id": "my_public_id",
  "version": "1",
  "name": "optional",
  "description": "optional",
  "citations": ["12345678"],
  "overwrite": "fail"
}
```

`id` and `version` are required; `overwrite` is one of `fail` (default),
`replace`, `merge`. Returns `201 Created` with a `Location` to the public
collection and a body naming the minted collection and members.

**`GET /api/v2/objects/{iri}/similar`** returns objects whose sequence aligns
to the target's, as a paginated envelope of hits carrying the alignment
measures.

### Collections

**`POST /api/v2/collections`** mints a submission into the authenticated
caller's own user namespace. This is v2's create verb for an SBOL document.
Accepts either a JSON body (the idiomatic path):

```json
{
  "id": "my_submission",
  "version": "1",
  "name": "My Submission",
  "description": "optional",
  "creator_name": "optional",
  "citations": ["12345678"],
  "format": "turtle",
  "content": "<the serialized SBOL document>",
  "overwrite": "fail"
}
```

or the same `multipart/form-data` upload v1 takes, so an existing SBOL file
rides through unchanged. `id` and `version` are required; `format` defaults to
RDF/XML; `overwrite` is `fail` (default), `replace`, or `merge`. Returns `201
Created` with a `Location` to the minted collection. An anonymous caller is
`403`.

### Search

**`GET /api/v2/search`** runs a ranked, ACL-scoped, paginated free-text query.
Typed query parameters, not the v1 path grammar:

| Parameter | Meaning |
| --- | --- |
| `q` | the free-text term; absent ranks the whole in-scope corpus |
| `type` | restrict to one rdf:type, given as a full IRI |
| `offset` | paging offset (default 0) |
| `limit` | page size, clamped to `[1, 1000]` (default 50) |

Returns the paginated envelope. Each hit carries `uri`, `display_id`,
`version`, `name`, `description`, and `object_type`.

**`POST /api/v2/search`** is the structured strategy surface. It accepts a
tagged query, typed filters, cursor paging, execution options, and an optional
strategy id. Omitting `strategy` uses the configured default. With a normal
zero-configuration server, that is `builtin.sbol-text-vector.v2`: a local,
checksum-pinned BGE-small vector index over canonical SBOL metadata. The image
ships the verified weights and the index rebuilds in the background at startup
then follows committed object writes, so newly started servers can return fewer
vector hits until the first rebuild finishes.
This route is additive: the compatibility routes and `GET /api/v2/search`
retain their existing ranking and wire contracts.

```json
{
  "strategy": "semantic.components.v1",
  "query": { "kind": "text", "text": "inducible promoter" },
  "filters": { "graphs": ["https://example.org/public"] },
  "page": { "limit": 20 },
  "options": { "explain": true }
}
```

**`GET /api/v2/search/strategies`** returns the default strategy and every
registered strategy's inputs, filters, pagination, totals, explanation,
data-egress, and dependency declarations. See
[Pluggable search](search-plugins.md) for SDK and deployment details.

### Sequences

**`GET /api/v2/sequences/search`** aligns a query nucleotide string against the
in-scope corpus. Parameters: `q` (or the alias `sequence`) for the query
string, `mode` (`global`, the default banded aligner, or `exact` substring),
and `limit`. Returns `{items, total}`; each hit carries the aligned object's
URI and metadata plus `percent_match`, `strand`, and `cigar`.

## OpenAPI

The v2 surface documents itself:

- `GET /api/v2/openapi.json`: the OpenAPI 3.1 spec.
- `GET /api/v2/docs`: an interactive reference rendered from that spec.

Both are public. The spec is the contract the conformance and OpenAPI tests
validate live responses against, so a handler that drifts from its documented
shape fails the build.

## Mapping from v1 to v2

The v2 surface covers the core read, write, search, and download paths of the
v1 SynBioHub-compat surface under idiomatic verbs. v1's federation, plugin,
admin, attachment-upload, and fine-grained field-edit routes have no v2
equivalent yet; use v1 for those.

| v1 (SynBioHub-compat) | v2 (idiomatic) |
| --- | --- |
| `GET /admin/theme` | `GET /api/v2/instance` (public safe subset) |
| `POST /login`, `POST /logout`, `GET /profile` | `POST`, `DELETE`, `GET /api/v2/session` (browser lifecycle) |
| `POST /submit` (multipart) | `POST /api/v2/collections` (JSON or multipart) |
| `GET /.../metadata` | `GET /api/v2/objects/{iri}` (`Accept: application/json`) |
| `GET /.../sbol`, `/sbolnr`, `/gb`, `/fasta`, `/gff`, `/omex` | `GET /api/v2/objects/{iri}?format=sbol\|sbolnr\|gb\|fasta\|gff\|omex` |
| SBOL2/SBOL3 negotiation via route conventions | `GET /api/v2/objects/{iri}?version=sbol2\|sbol3` or `Accept` |
| `GET /.../remove` or `POST /.../remove` | `DELETE /api/v2/objects/{iri}` |
| `POST /user/.../makePublic` | `POST /api/v2/objects/{iri}/publish` |
| `POST /updateMutableDescription`, `/updateMutableNotes`, `/updateMutableSource`, `/updateCitations`, `/.../edit/:field` | `PATCH /api/v2/objects/{iri}` |
| `GET /search/<term>`, `GET /search/key=value&...` | `GET /api/v2/search?q=<term>&type=<iri>&limit=&offset=` |
| `GET /rootCollections`, `GET /:type/count`, `GET /searchCount/...` | `GET /api/v2/objects` (paginated envelope carries `total`) |
| `GET /.../similar` | `GET /api/v2/objects/{iri}/similar` |
| sequence search (SBOLExplorer plugin) | `GET /api/v2/sequences/search` |
| `X-authorization: <token>` | `Authorization: Bearer <token>` or the HttpOnly browser cookie |
| per-shape bodies, SPARQL-results JSON | one JSON envelope, `{items, total, offset, limit}` |
| per-route error handling | `{error: {code, message, status}}` |
