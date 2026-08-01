# SynBioHub compatibility and cutover matrix

SBOL DB is the product. SynBioHub behavior in this document is a maintained
compatibility contract for existing clients and migrated deployments; it is
not the application name, visual identity, or frontend architecture.

This matrix is the cutover record required before any compatibility route can
be considered for retirement. It distinguishes wire compatibility from native
V2 product behavior and keeps every removal behind an explicit release
decision.

## Complete endpoint inventory

The deployed compatibility surface has three checked parts:

1. [`synbiohub_openapi.json`](../crates/sbol-db-server/src/synbiohub_openapi.json)
   is the primary catalog: 109 paths and 127 method/path operations. Every
   operation has exactly one of the workflow-family tags mapped below.
2. [`synbiohub-compatibility-aliases.txt`](synbiohub-compatibility-aliases.txt)
   lists 61 additional maintained route patterns. These are version-less,
   share-link, browser-support, job-support, or deprecated remote aliases and
   intentionally do not make the primary catalog harder to navigate.
3. `GET/POST /sparql-auth` and
   `GET/POST/PUT/DELETE /sparql-graph-crud-auth[/]` are the two
   Virtuoso-protocol compatibility paths. They use HTTP Basic authentication,
   not the application token transport.

Together these are 170 distinct deployed API path patterns. The inventory
contract test parses the actual Axum route declarations and fails when a route
is added, removed, or moved without updating one of the two catalogs. Unknown
extension paths are not compatibility endpoints and retain the normal JSON
404. The transitional `/lab/*` browser mount is measured separately because it
is a bookmark surface, not an API.

## Primary workflow families

“Equivalent” below means the domain result is equivalent while V2 uses an
idiomatic JSON/HTTP shape. It does not promise byte-identical JSON, HTML, RDF
serialization, ranking scores, or incidental classic error bugs.

| Primary OpenAPI family | Compatibility status | Classic transport and behavior | Native replacement and frontend owner | Evidence class |
| --- | --- | --- | --- | --- |
| Auth | Supported | Form bodies and `X-authorization`; legacy hashes upgrade after a successful login. | `/api/v2/session`, `/api/v2/account`; `/login`, `/account`. Equivalent identity with HttpOnly-cookie or bearer transport. | Auth/session HTTP tests and backend-neutral identity conformance. |
| Query | Supported, ranking intentionally different | Path/query-string search grammar, counts, root collections, metadata and relationship reads. | `/api/v2/search`, facets, object details and sequence search; `/search`, `/sequence-search`, object pages. Exact filtering/paging where specified; native ranking is not Elasticsearch score parity. | Differential cases, full-corpus discovery gate, stable-paging tests. |
| Downloads | Supported, semantically equivalent | Object suffixes negotiate SBOL, non-recursive SBOL, GenBank, FASTA, GFF3, OMEX, summary, and full views. V1 RDF defaults to classic SBOL 2 and accepts explicit SBOL 3. | `/api/v2/objects/{iri}` content negotiation and direct machine links from the object page. Native V2 RDF defaults to SBOL 3 and accepts explicit SBOL 2. | RDF isomorphism, parser, archive-member, backend round-trip tests, and the live three-subject differential. |
| Submission | Supported compatibility | Multipart `POST /submit`, classic identity minting, overwrite modes, and collection consequences. | Validate-first `/api/v2/collections/validate` then `/api/v2/collections`; `/contribute`. Equivalent persisted graph semantics with a modern preview contract. | Submission conformance and V2 lifecycle tests. |
| Edit | Supported compatibility | Classic mutable-field and path-shaped edit/add/remove forms. | `PATCH /api/v2/objects/{iri}` and collection member commands; object and collection management pages. V2 accepts only modeled fields. | Positive/forbidden mutation and provenance tests. |
| Permissions | Supported; V2 intentionally separates concepts | V1 `addOwner` preserves classic co-owner semantics. | V2 read-only shares, revocation, ownership transfer, and curator review are distinct commands; private object collaboration UI. | Complete role matrix and append-only audit tests. |
| Attachments | Supported compatibility | Multipart upload, URL attachment, and hash download routes. | Existing machine routes remain; native object details render attachment metadata. A native upload replacement is not yet claimed. | Blob and both-vocabulary attachment conformance. |
| Admin | Supported compatibility; presentation intentionally different | `X-authorization`; several classic HTML pages return structured JSON in SBOL DB. | `/api/v2/admin/*`; `/admin/*`. One administrator policy, redacted secrets, deliberate confirmations, and audit. | Admin HTTP tests plus SQLite/RocksDB/Postgres storage conformance. |
| Plugins | Capability-constrained | Configured HTTP plugin proxy, expose, and stream handoffs are retained. | Native integration management is `/api/v2/admin/plugins`; there is no native arbitrary-code execution surface. | Plugin proxy and administrator-policy tests. |
| SPARQL | Supported protocol compatibility | Basic-auth query/update and graph-store shapes expected by classic deployments. | Native `/sparql` and graph/object APIs for new clients. | Graph protocol, auth, and differential SPARQL tests. |

## Supplemental aliases

The checked supplemental file is the exact method-independent path list. Its
families have these policies:

| Alias family | Status | Replacement |
| --- | --- | --- |
| Version-less public/user identities and relationship/download suffixes | Supported compatibility | Follow the resolved canonical identity or use `/api/v2/objects/{iri}`. |
| Hash-scoped share routes and `shareLink` | Supported compatibility | Native read-only collaboration uses V2 shares; existing hashes remain readable. |
| `/setup`, `/browse`, autocomplete, DataTables, result stream, job/corrupt-log, and additional admin aliases | Compatibility-only | Native setup, search, workspaces, jobs, and activity pages own new browser traffic. No retirement date is set. |
| `/remoteLogin`, `/remoteSearch[/…]`, `/remoteSubmit[/]` | Deprecated alias, still supported | Use the canonical login/search/submit compatibility route or the corresponding V2 workflow. |
| `/lab` and `/lab/*` browser bookmarks | Transitional, still supported | The client maps them to `/admin` routes. Usage is counted without recording the deep-link value. |

## Intentional differences and unsupported behavior

| Behavior | Classification | Reason and fallback |
| --- | --- | --- |
| Known browser page routes with `Accept: text/html` | Intentionally different | The SBOL DB React application owns the page; non-HTML requests and reserved object suffixes continue to the compatibility handler. |
| Classic product name, theme colors, and server-rendered page chrome | Intentionally different | Native pages retain SBOL DB branding and semantic design tokens. Deployment name remains secondary instance context. |
| Exact Elasticsearch/SBOLExplorer ranking scores and order | Intentionally different | V1 query shapes remain accepted, but native ranked text is the development default and BGE is an explicit opt-in release gate. Filtering, authorization, totals, and stable paging are the compatibility claims. |
| Classic malformed-request crashes or permissive secret handling | Intentionally different | SBOL DB returns bounded errors, rejects invalid confirmations, and never reproduces a security bug for byte parity. |
| Local Node module execution from classic plugin directories | Unsupported | It would introduce a new code-execution trust model. Only explicitly configured HTTP integrations are supported. |
| Outgoing password-reset/notification mail without a configured delivery capability | Unsupported for delivery | Reset controls remain capability-gated; tokens are never exposed to frontend JavaScript as a substitute. |
| Uncataloged third-party extension routes | Unsupported by default | They receive JSON 404 unless deliberately implemented and classified. A reverse proxy may continue to own deployment-specific extensions. |

## Authentication and privacy

- V1 application routes accept the classic `X-authorization` token. V2 accepts
  `Authorization: Bearer` and the same token in an HttpOnly browser cookie.
- Cookie-authenticated mutations enforce same-origin browser metadata.
- The Virtuoso protocol uses its documented Basic credentials and is not
  silently treated as an application session.
- `sbol_db_compatibility_requests_total` exports only fixed `surface`,
  `family`, HTTP method, and status labels. It never exports a raw path, search
  expression, IRI, username, header, or body.
- `sbol_db_legacy_ui_requests_total` exports only `root` or `deep_link`, method,
  and status. The bookmark itself is not a label.

## Retirement decision record

No route is proposed for removal by this phase. A future proposal must include
all of the following before implementation:

1. a bounded metric query and observation window showing measured use;
2. an available replacement and migration example;
3. release notice and a date no earlier than the documented support window;
4. a rollback that restores the exact route and persisted semantics; and
5. an explicit product decision approving the removal.

Until then, telemetry is evidence only. It does not trigger automatic
deprecation or deletion.
