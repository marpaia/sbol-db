# The SynBioHub v2 API

The SynBioHub v2 API is SBOL DB's idiomatic, RESTful presentation of the
SynBioHub product surface, mounted under `/api/v2`. Every v2 handler calls the
same application facade verbs as the SynBioHub v1 compatibility adapter; the
two surfaces read and write one dataset, one identity model, and one ACL scope.
V2 holds no business logic of its own. It differs from v1 only in wire shape.

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
mapping the SBOL DB API uses, so the two surfaces never diverge on the meaning
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
    "profile_management": true,
    "password_change": true,
    "password_reset": false,
    "collaboration": true,
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

### Account and collaboration

**`GET /api/v2/account`** returns the authenticated caller's safe profile.
**`PATCH /api/v2/account`** updates only `name` and `affiliation`; identity,
email, graph URI, and role flags are deliberately immutable through
self-service. Both responses carry `Cache-Control: no-store` and never include
password or token material.

**`POST /api/v2/account/password`** requires `current_password` and
`new_password`, re-verifies the current credential, and stores a fresh Argon2
hash. Password reset remains capability-gated (`password_reset: false`) until
an external delivery worker is selected and configured; the portal therefore
does not mint or expose an undeliverable reset token.

**`GET /api/v2/account/shared`** returns the exact objects explicitly shared
with the caller. These projections pass through the same ACL-aware object
details service as ordinary object pages and are sorted by canonical IRI.

Native collaboration intentionally separates read access from ownership:

- **`GET /api/v2/objects/{iri}/shares`** lists owners and read-only viewers for
  an owner or administrator.
- **`POST /api/v2/objects/{iri}/shares`** grants an active member read-only
  access without adding an ownership stamp.
- **`DELETE /api/v2/objects/{iri}/shares/{user}`** revokes that read-only
  access immediately.
- **`PUT /api/v2/objects/{iri}/owner`** atomically transfers the caller's
  ownership stamps. An administrator who is not an owner cannot silently use
  this self-service operation to reassign the object.

This is a deliberate modern wire distinction from classic `addOwner`, whose
compatibility semantics still create a co-owner. The classic adapter remains
unchanged.

Curator review is append-only:

- **`POST /api/v2/objects/{iri}/reviews`** assigns an active curator, grants
  them read-only access, and records `review_requested` in the same atomic
  update.
- **`POST /api/v2/objects/{iri}/reviews/decision`** lets the assigned curator
  or an administrator append `review_approved` or
  `review_changes_requested`.
- **`GET /api/v2/objects/{iri}/reviews`** returns the latest review cycle to
  its participants, an owner, or an administrator.
- **`GET /api/v2/reviews`** returns cases requested by or assigned to the
  caller; administrators see the complete queue.
- **`GET /api/v2/objects/{iri}/activity`** returns owner/admin audit evidence
  for first-class shares, revocations, transfers, and review decisions.

Audit events are immutable RDF resources stored in the object's graph.
SynBioHub v2 object mutations and their evidence are composed into one SPARQL Update, so a
successful state transition cannot be committed without its audit event.

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

**`GET /api/v2/objects/{iri}/details`** is the normalized object-page resource.
Unlike the storage-oriented JSON representation above, it combines the
authorized subject projection with inverse collection membership, uses,
exact-sequence twins, attachment metadata, provenance, sequence content, and
the exact RDF property set. Every biological section carries an explicit
`available`, `empty`, `partial`, or `unsupported` state so clients never need
to infer support from missing JSON fields. It also reports the logical source
graph and only returns a persisted content fingerprint when the selected
projection corresponds to the stored record. Unknown and out-of-scope objects
both return the same non-disclosing `404`.

This resource is the contract consumed by the public object page. Biological,
identity, visibility, and graph-scope interpretation remains in
`sbol-db-app`; the React client renders the typed projection and treats all
imported prose and links as untrusted input.

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
rides through unchanged. `id` and `version` are required; `format` is one of
`rdfxml`, `turtle`, `jsonld`, `ntriples`, `genbank`, or `fasta` and defaults to
RDF/XML. GenBank and FASTA are validated and converted to SBOL 3 before URI
minting. `overwrite` is `fail` (default), `replace`, or `merge`. Returns `201
Created` with a `Location` to the minted collection. An anonymous or
non-member caller is `403`.

**`POST /api/v2/collections/validate`** accepts the identical JSON or multipart
request but performs no write. It parses and validates the source, reports any
SBOL 3 conversion, mints the exact identities commit would use, checks the
target graph for a collision, and returns the resulting `create`,
`reject_conflict`, `replace`, or `merge` consequence. Commit repeats the same
preparation authoritatively, so a race cannot bypass validation.

**`POST /api/v2/collections/{iri}/members`** adds the IRI in a JSON
`{"member":"…"}` body to an owned Collection. **`DELETE
/api/v2/collections/{iri}/members/{member}`** removes that membership. Both
return `204`; an anonymous or non-owner caller is `403`.

**`DELETE /api/v2/collections/{iri}`** removes an owned Collection and its
submission closure, returning `204`. The broader collection deletion is kept
separate from `DELETE /objects/{iri}`, which removes only the selected
top-level closure. The account UI names the exact target and requires a typed
confirmation before calling this operation.

### Search

**`GET /api/v2/search`** runs normalized, ACL-scoped registry discovery. Typed
query parameters replace the v1 path grammar:

| Parameter | Meaning |
| --- | --- |
| `q` | the free-text term; absent browses the whole in-scope corpus |
| `type` | restrict to one rdf:type, given as a full IRI |
| `role` | restrict to one SBOL 2 or SBOL 3 role, given as a full IRI |
| `collection` | restrict to direct members of a collection IRI |
| `owner` | restrict to an exact `sbh:ownedBy` graph IRI |
| `provenance` | case-insensitive substring of mutable provenance |
| `created_after`, `created_before` | inclusive creation dates in `YYYY-MM-DD` form |
| `modified_after`, `modified_before` | inclusive modification dates in `YYYY-MM-DD` form |
| `sort` | `relevance`, `name`, `created`, `modified`, or `iri` |
| `direction` | `asc` or `desc`; omitted uses the sort's natural direction |
| `offset` | paging offset (default 0) |
| `limit` | page size, clamped to `[1, 1000]` (default 50) |

Returns an exact `total` and deterministic page ending in an ascending IRI
tie-breaker. Each hit carries `uri`, `display_id`, `version`, `name`,
`description`, `object_type`, `roles`, `owners`, creation/modification dates,
and its relevance `score`. The envelope also echoes the effective `sort` and
`direction`.

**`GET /api/v2/search/facets`** returns exact type and role counts for the
caller's visible corpus. Values carry their full IRI, display label, optional
CURIE, and count. See the [registry discovery contract](discovery-contract.md)
for URL state, semantics, and classic-link translation.

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

### Administration

The SynBioHub v2 administrator control plane is rooted at
`/api/v2/admin`. Every route below is protected by the same policy: an
anonymous caller receives 401, a signed-in non-administrator receives 403,
and hiding a link in React is never the authorization boundary. Browser
requests may authenticate with the HttpOnly session cookie; API clients may
use the same bearer token accepted elsewhere in V2.

| Section | Routes | Contract |
| --- | --- | --- |
| Overview and instance | `GET /admin`, `GET/PATCH /admin/instance` | Safe capability/status projection and typed instance policy. |
| RDF catalog | `GET /admin/dashboard`, `/admin/graphs*`, `/admin/resources*`, `/admin/sequences` | One backend-independent, cursor-paged view over canonical RDF. Graph/resource/sequence counts and rows do not depend on optional typed-import tables. |
| Accounts | `GET/POST /admin/users`, `PATCH/DELETE /admin/users/{username}` | Storage-side `q`, `limit`, and `offset` pagination; secret-free account projections; self-delete, self-demotion, and final-administrator removal are rejected. `is_member` is an explicit source role, not a migration marker. |
| Integrations | `GET /admin/integrations`, federation sync/join, registry, remote, and plugin mutation routes | Backend-neutral durable configuration. Secret-shaped remote fields are recursively redacted before serialization; federation join/sync and runtime plugin calls are the external network operations. |
| Jobs and ontology | `/admin/jobs*`, `GET/POST /admin/ontologies` | Read/enqueue/cancel and ontology loading without using the unscoped SBOL DB API endpoints from the UI. |
| Search | `GET /admin/search`, `POST /admin/search/rebuild` | Capability-aware strategy status and a coalesced rebuild command. |
| Complete backup | `GET/POST /admin/backup` | Status/history and enqueueing for the same encrypted RocksDB, blob, search, and ACME checkpoint used by manual, scheduled, and pre-deploy triggers. Available in the self-contained production profile. |
| Edge runtime | `GET/PATCH /admin/edge` | Active and pending production settings, restart-required state, TLS/ACME/disk health, and bounded offline recovery history. Secrets and recovery identities are not accepted. |
| Audit | `GET /admin/audit` | Newest-first append-only administrator events. |

Destructive requests carry a `confirmation` value that must exactly name the
target, such as `DELETE <username>` or `CANCEL JOB <uuid>`. Administrator
actions append an attempt/success/failure event to the dedicated audit graph.

There is one production disaster-recovery artifact, not a graph-only admin
archive. It contains a native consistent RocksDB checkpoint (including durable
configuration and account state), attachment blobs, search state, and ACME
state. Success requires local semantic verification, encrypted upload, remote
readback, and a second semantic verification. Verification, restore, and
rollback are offline CLI operations because the recovery identity and atomic
generation activation must remain outside the running web process.

## OpenAPI

The v2 surface documents itself:

- `GET /api/v2/openapi.json`: the OpenAPI 3.1 spec.
- `GET /api/v2/docs`: an interactive reference rendered from that spec.

Both are public. The spec is the contract the conformance and OpenAPI tests
validate live responses against, so a handler that drifts from its documented
shape fails the build.

## Mapping from v1 to v2

The v2 surface covers the core read, write, search, download, collaboration,
and administrator paths of the v1 compatibility surface under idiomatic verbs.
Attachment upload and a few fine-grained classic field-edit shapes remain V1
compatibility operations; SBOL DB product pages do not call them indirectly.
Legacy V1 `/sbol` and `/sbolnr` downloads default to SBOL 2 for classic client
compatibility and accept `?version=sbol3` explicitly. SynBioHub v2 RDF downloads
default to SBOL 3 and accept `?version=sbol2` when a legacy representation is
required.
The complete path inventory, parity classifications, deprecated aliases, and
unsupported behaviors live in the
[compatibility and cutover matrix](synbiohub-compatibility-matrix.md).

| v1 (SynBioHub-compat) | v2 (idiomatic) |
| --- | --- |
| `GET /admin/theme` | `GET /api/v2/instance` (public safe subset) |
| V1 `/admin/*` instance, user, integration, job, ontology, and search operations | `/api/v2/admin/*` (one administrator policy and JSON contracts) |
| `POST /login`, `POST /logout`, `GET /profile` | `POST`, `DELETE`, `GET /api/v2/session` (browser lifecycle) |
| `POST /submit` (multipart) | `POST /api/v2/collections` (JSON or multipart) |
| `GET /.../metadata` | `GET /api/v2/objects/{iri}` (`Accept: application/json`) |
| object page metadata assembled from multiple classic endpoints | `GET /api/v2/objects/{iri}/details` |
| `GET /.../sbol`, `/sbolnr`, `/gb`, `/fasta`, `/gff`, `/omex` | `GET /api/v2/objects/{iri}?format=sbol\|sbolnr\|gb\|fasta\|gff\|omex` |
| SBOL2/SBOL3 negotiation via route conventions | `GET /api/v2/objects/{iri}?version=sbol2\|sbol3` or `Accept` |
| `GET /.../remove` or `POST /.../remove` | `DELETE /api/v2/objects/{iri}` |
| `POST /user/.../makePublic` | `POST /api/v2/objects/{iri}/publish` |
| `POST /updateMutableDescription`, `/updateMutableNotes`, `/updateMutableSource`, `/updateCitations`, `/.../edit/:field` | `PATCH /api/v2/objects/{iri}` |
| `GET /search/<term>`, `GET /search/key=value&...` | `GET /api/v2/search?q=<term>&type=<iri>&role=<iri>&collection=<iri>&owner=<iri>&provenance=<text>&created_after=&created_before=&modified_after=&modified_before=&sort=&direction=&limit=&offset=` |
| `GET /rootCollections`, `GET /:type/count`, `GET /searchCount/...` | `GET /api/v2/objects` (paginated envelope carries `total`) |
| `GET /.../similar` | `GET /api/v2/objects/{iri}/similar` |
| sequence search (SBOLExplorer plugin) | `GET /api/v2/sequences/search` |
| `X-authorization: <token>` | `Authorization: Bearer <token>` or the HttpOnly browser cookie |
| per-shape bodies, SPARQL-results JSON | one JSON envelope, `{items, total, offset, limit}` |
| per-route error handling | `{error: {code, message, status}}` |
