# SBOL DB application architecture and compatibility roadmap

This document is the technical source of truth for turning the former Data Lab
into the user-facing SBOL DB application. It describes the runtime boundary,
SynBioHub compatibility policy, frontend architecture, quality gates, and the
order in which the remaining compatible workflows should move.

The measurable definition of completion, autonomous delivery boundary, and
required evidence for every increment live in the
[application acceptance contract](application-acceptance.md).

The target is one product with two deliberately different surfaces:

- a public and account-aware registry at `/`; and
- an administrator-only data and operations workspace at `/admin`.

Both surfaces are one React application, embedded into and served by the same
Axum process as the APIs. There is no browser-only backend and no second source
of domain truth.

## Product and compatibility invariants

1. SBOL identities remain stable. A canonical `/public/*` or `/user/*` URL can
   render a browser page without changing the identity or its RDF and download
   subresources.
2. Existing V1 SynBioHub, Virtuoso-compatible, native `sbol-db`, and V2 clients
   must continue to reach machine handlers. A missing or wildcard `Accept`
   header is never treated as SPA navigation.
3. Presentation code does not reimplement identity, ACL, search, publishing, or
   storage rules. React consumes typed HTTP contracts; adapters delegate to
   `AppServices`.
4. SBOL DB is the product identity on every native page. A deployment name is
   secondary context. Classic SynBioHub names and visual-theme values remain
   confined to explicit compatibility, migration, conformance, and import
   boundaries; they never recolor or rename the native application.
5. Admin status is enforced on the server for admin APIs. Hiding a route or
   navigation item is only a usability affordance, never the security boundary.
6. Search evaluation covers the complete available corpus. Small fixtures are
   useful smoke tests, but they are not evidence that registry discovery works.
7. A UI migration phase is complete only when its routes, API behavior,
   accessibility, responsive layout, loading/error/empty states, and
   compatibility probes are all verified.

## Runtime topology

```text
Browser or API client
        |
        v
single sbol-db origin
        |
        +-- compatibility-aware page dispatcher
        |     +-- exact embedded asset -> asset response
        |     +-- known page + explicit HTML -> React index
        |     `-- everything else -> Axum router
        |
        +-- /api/v2/*             native SBOL DB application API
        +-- SynBioHub V1 paths    compatibility adapter
        +-- native sbol-db paths  storage/query/worker API
        +-- /lab/api/*            protected admin-workspace API
        `-- /admin/*              lazy React admin application
```

The dispatcher is an allowlist, not a fallback. Unknown paths must reach Axum
so extensions and future compatibility endpoints are not silently converted
into HTML. It handles only `GET` and `HEAD`; mutations always reach their API
handler. `/assets/*` is exact-match only, so a missing JavaScript file returns
404 instead of the SPA index.

### Route ownership

| Route family | Browser HTML owner | Machine owner | Notes |
| --- | --- | --- | --- |
| `/`, `/search/*`, `/sequence-search` | React portal | existing handler when HTML is not preferred | Search accepts canonical query-string state; classic path grammar is translated visibly and sequence grammar moves to its dedicated workflow. |
| `/connect` | React portal | none | Public guide for connecting the `sbol` CLI, V2 REST clients, and MCP-capable agents; registry permissions still govern every request. |
| `/login`, `/register`, `/setup` | React portal | V1 handler for mutations and non-HTML requests | V2 session is preferred by the new UI; V1 remains compatible. |
| `/account`, `/workspace/*`, `/contribute` | React portal | none | Native account, collaboration, review, and validate-first contribution workflows. |
| `/objects/view/:iri` | React portal | none | URI is encoded as one route parameter. |
| `/public/*`, `/user/*` | React canonical redirect/object page | V1 identity and representation handlers | Reserved suffixes such as `/sbol`, `/full`, `/uses`, and `/download` always remain APIs. |
| `/api/v2/*` | V2 docs only | V2 adapter | Never intercepted by the portal dispatcher. |
| `/admin/*` | React admin application | V1 `/admin/*` requests when HTML is not preferred | The UI route is session/admin gated. |
| `/lab/*` | transitional React entry | `/lab/api/*` | Old bookmarks redirect to the matching `/admin/*` route. |
| all other paths | none unless explicitly added | existing Axum handler/fallback | New page families require a dispatch test before being added. |

`SBOL_DB_PORTAL_ENABLED=false` disables public registry-page interception
without removing APIs; the admin shell, login/setup entry points, assets, and
transitional `/lab` mount remain. `SBOL_DB_LAB_ENABLED=false` removes both the
embedded application and its `/lab/api` surface. A build without the `lab`
feature strips the frontend assets from the server entirely.

## HTTP contracts and authentication

### Application bootstrap

`GET /api/v2/instance` is the safe, cache-revalidated bootstrap document. It
contains safe deployment context, setup state, public access policy, and
capability flags. It must never grow into a dump of `/admin/theme`; adding a
field requires an explicit decision that it is safe before authentication.
In particular, legacy theme colors are not part of this contract.

`GET`, `POST`, and `DELETE /api/v2/session` provide one browser-session model:

- credentials are exchanged for the same opaque token understood by the V1
  adapter;
- the token is stored only in an `HttpOnly; SameSite=Lax; Path=/` cookie;
- production HTTPS deployments enable the `Secure` attribute;
- bearer authorization takes precedence over ambient cookies;
- cookie-authenticated mutations require same-origin browser metadata; and
- logout revokes the selected token before clearing the cookie.

The session response contains a safe user projection and never a credential,
password hash, reset token, or other authentication secret.

### Access policy

When persisted instance configuration enables `requireLogin`, anonymous V2
resource requests return 401. Version, instance, session, OpenAPI, and docs
remain available so the portal can bootstrap and offer sign-in.

`/admin/*` has two gates:

1. React redirects anonymous users to `/login?next=...` and gives signed-in
   non-admin users a clear permission page.
2. `/api/v2/admin/*` and the retained `/lab/api/*` workbench surface resolve
   the same bearer/cookie identity and return 401 for an anonymous caller or
   403 for a non-admin caller.

The native admin control plane now lives entirely under `/api/v2/admin/*` and
uses one `require_admin` policy. Instance configuration, accounts,
integrations, jobs, ontology loading, search maintenance, backup/restore, and
the admin audit stream all use that boundary. The older root management and V1
compatibility routes retain their existing contracts for compatible clients;
they are classified migration inputs, not authority for the native UI. Changing
or removing one remains an endpoint-by-endpoint compatibility decision.

### Compatibility discipline

V1 compatibility and V2 product APIs are two presentations of the same
application services. New portal behavior belongs in application/domain code
when it changes semantics, and in the V2 adapter when it changes only the wire
contract. The compatibility adapter should not call V2 handlers, and V2 should
not scrape or translate V1 responses.

For every migrated workflow, maintain an endpoint matrix with:

- V1 method/path and accepted request encodings;
- V2 method/path and schema;
- authentication and ACL behavior;
- status, headers, and representation behavior;
- the owning frontend route; and
- conformance evidence against classic SynBioHub where parity is intended.

The implemented search request, public URL state, response shape, and classic
path translation are specified in the
[registry discovery contract](discovery-contract.md).

## Frontend architecture

### Route shells

`App.tsx` owns product topology only:

- `PublicShell` provides SBOL DB identity, optional deployment context, primary navigation, account/theme
  controls, setup status, footer, and the public route outlet.
- `PublicAccessGate` mirrors `requireLogin` for a coherent browser transition;
  the server remains authoritative.
- `AdminGate` resolves the session and role before mounting `LabLayout`.
- Existing Data Lab pages are lazy route elements beneath `/admin`, so public
  visitors do not download the workbench, Monaco, observability, or maintenance
  code. Each admin page is loaded only when visited.

The legacy `/lab/*` redirect is kept in one route helper. Components must use
`adminPath()` and `publicObjectPath()` rather than assembling cross-surface URLs
ad hoc.

### Feature and data boundaries

Frontend code is organized in four layers:

| Layer | Responsibility |
| --- | --- |
| `components/ui` | ShadCN/Radix primitives and project-wide variants. No domain fetching. |
| `components/portal`, `components/admin`, and `components/lab` | Reusable composed presentation for one surface. |
| `features/portal`, `features/admin`, and hooks | Typed HTTP clients, query keys, mutations, formatting, and domain-oriented client state. |
| `routes` | Page composition, URL state, and workflow orchestration. |

TanStack Query owns server state. URL parameters own shareable search/filter and
pagination state. Local component state is limited to transient form and menu
state. No route should create an alternative global cache for server data.

### Design-system direction

The existing ShadCN/Radix and Tailwind stack remains the design-system base.
Investment should concentrate on consistent primitives rather than one-off page
styling:

- semantic color tokens for background, foreground, borders, status, focus,
  SBOL categories, and sidebar surfaces;
- a fixed, contrast-tested SBOL DB palette across accessible light/dark token bands;
- shared typography, spacing, radius, elevation, empty-state, and data-density
  conventions;
- cards, badges, search controls, result summaries, metadata rows, tables,
  pagination, dialogs, and feedback states assembled from the primitives;
- visible focus, semantic landmarks and labels, keyboard operation, and reduced
  motion as component acceptance criteria; and
- brief, property-specific transitions, origin-aware menus, and tactile active
  states without page-load animation or gratuitous motion.

SBOL-specific visualizations should be reusable domain components with a text
and metadata fallback. They must clarify the biological object, not act as
decoration.

## Delivery sequence

### Slice 0: application foundation — implemented

- root-mounted, compatibility-aware portal serving;
- exact asset handling and transitional `/lab/*` bookmarks;
- public instance/session contracts and shared V1/V2 browser tokens;
- setup, sign-in, registration, `requireLogin`, and admin-role gates;
- public home, registry search, initial object detail/download view;
- Data Lab relocation to `/admin` with protected `/lab/api` operations;
- ShadCN/Tailwind application shell, responsive states, SBOL DB design tokens, and lazy
  admin bundles; and
- focused HTTP, schema, auth, feature-gate, build, and browser verification.

### Slice 1: discovery parity

- Define a normalized V2 search request supporting text, SBOL class, role,
  collection, owner, provenance, created/modified ranges, and sorting.
- Replace the free-form type IRI field with ontology-backed facets while
  retaining a precise advanced control.
- Add result-density controls, stable pagination, URL serialization, and
  accessible result summaries.
- Support classic path-style search links and document every translation that
  is intentionally lossy.
- Evaluate ranking, filtering, and pagination over the full available corpus;
  use small fixtures only as deterministic smoke checks.

Exit gate: the selected SynBioHub 2 compatibility journeys are reproducible by
URL, no visible object is omitted by pagination, and API/UI counts agree.

### Slice 2: object understanding

- Build an object-page frame for identity, display ID/version, type, roles,
  provenance, ownership/visibility, citations, and source graph.
- Add reusable sections for sequence, components/features, interactions,
  attachments, collections, `uses`, `twins`, and similarity.
- Preserve direct RDF/SBOL/GenBank/FASTA/GFF/OMEX downloads as machine links;
  never route their suffixes through React.
- Add SBOL Visual rendering only with explicit unsupported/partial states and a
  metadata-first fallback.

Exit gate: canonical V1 identities open the modern page, every advertised
download is byte/semantic checked, and unsupported biology is visible rather
than silently dropped.

The implemented object-page boundary is
`GET /api/v2/objects/{iri}/details`. `sbol-db-app` selects the authorized
logical/physical graph pair and assembles identity, visibility, provenance,
sequence, relationship, attachment, and lossless RDF-property sections. The
frontend consumes those explicit states and never reconstructs biological or
ACL semantics from a storage record. Download routes remain separate machine
representations; HTTP tests exercise every format and backend conformance tests
round-trip the sequence-bearing formats.

### Slice 3: contribution and collection workflows

- Introduce account-scoped workspace and collection routes.
- Add validate-first submission for SBOL 2, SBOL 3, GenBank, and FASTA with an
  explicit import report before commit.
- Support collection creation, editing, membership, ownership, publishing, and
  removal through application-service commands and V2 contracts.
- Keep raw graph import as a clearly labeled administrator workflow; ordinary
  contribution should use identity/ACL-aware application operations.

Exit gate: create, inspect, revise, publish, and download form one tested
workflow with provenance and ACL assertions at every transition.

The implemented member workflow is `/contribute` → `/workspace` → the shared
object page. `POST /api/v2/collections/validate` and `POST
/api/v2/collections` accept the same contract, while only the second writes.
SBOL 2/3 RDF stays in its asserted standard; GenBank and FASTA conversion to
SBOL 3 is explicit in the preview. Collection metadata, membership,
publication, and removal call owner-gated application commands through V2.
The HTTP lifecycle test proves write-free cancellation, non-disclosing private
reads, forbidden non-owner edits, provenance through revision/publication, and
an anonymously parseable public download.

### Slice 4: accounts and collaboration — implemented

- Profile and affiliation management, password changes/resets, account graph,
  ownership transfer, shared collections, and curator review queues.
- Decide and document outbound-email requirements before exposing reset or
  notification controls.
- Add session/device revocation if token lifetime or multi-session management is
  introduced.

Exit gate: anonymous, member, curator, and administrator journey tests cover
both positive and forbidden cases.

The native account surface is `/account`, backed by no-store V2 profile and
current-password-gated password commands. Reset remains capability-disabled
until mail delivery exists. `/workspace` separates owned, shared, and review
queues. Read-only shares use `sbh:canView` without granting ownership; transfer
is a distinct atomic command and classic co-owner behavior remains confined to
the V1 adapter. Review requests atomically share the object with an active
curator and append an immutable RDF audit event. Decisions append rather than
rewrite history. Object activity exposes the same event stream only to an
owner or administrator. HTTP tests cover anonymous, unrelated member, owner,
recipient, revoked recipient, curator, and administrator powers, and OpenAPI
response tests exercise the account, sharing, review, and activity schemas.

### Slice 5: admin control-plane consolidation — implemented

- Move instance/theme policy editing, users, remotes, plugins, search index,
  jobs, ontology loading, backup/restore, and maintenance into coherent admin
  sections.
- Put every privileged read and mutation behind one documented admin auth
  policy, including the native endpoints currently used by the Data Lab.
- Separate safe operational status from destructive actions; require explicit
  confirmation and audit records for destructive operations.
- Keep capability-driven degradation for SQLite, RocksDB, and Postgres.

Exit gate: an unauthenticated or member account cannot call any control-plane
operation directly, and all administrator actions have audit and failure-path
evidence.

The implementation is a native `/api/v2/admin/*` boundary with one
administrator middleware and a typed frontend client. It separates read-only
status from mutations; recursively redacts remote secrets; protects account
administration against self-deletion, self-demotion, and removal of the final
administrator; and requires exact target-bearing confirmations for destructive
actions. Backup archives are canonical, checksum-verified registry-graph
snapshots and restore atomically before search maintenance is queued. The
append-only RDF audit graph records attempted, successful, and failed admin
actions. Backend-neutral user/config/storage conformance and HTTP tests cover
the policy, redaction, destructive guards, backup integrity, and audit results.

### Slice 6: compatibility cutover and retirement — implemented

- Run the maintained V1 conformance suite and browser/API collision matrix on
  SQLite, RocksDB, and Postgres.
- Add deployment telemetry for V1 endpoint usage and legacy `/lab` redirects
  before setting any retirement date.
- Publish a compatibility/deprecation table and migration guide for API and
  bookmark consumers.
- Remove a compatibility path only after measured usage, documented notice,
  and an explicit release decision.

The maintained compatibility boundary now has a checked inventory: 109 primary
OpenAPI paths, 61 supplemental aliases, and two Virtuoso protocol paths. Fixed
family telemetry measures compatibility and legacy `/lab` bookmark use without
recording raw paths, IRIs, searches, users, headers, or bodies. The V1 subject
suite, browser/API collision matrix, and synthetic classic-instance migration
rehearsal run across SQLite, RocksDB, and Postgres. A clean live differential
against classic SynBioHub covers the Elasticsearch-independent read surface on
all three subjects. Legacy V1 SBOL downloads default to SBOL 2 as classic
clients expect, while explicit `?version=sbol3` and native V2 downloads retain
the modern SBOL 3 path. The compatibility matrix classifies every supported,
deprecated, intentionally different, and unsupported behavior. No route is
removed and no retirement date is set by this slice.

## Verification matrix

Every slice should add evidence at the lowest stable layer and one end-to-end
journey through the production-shaped server:

| Boundary | Required evidence |
| --- | --- |
| Domain/application | Backend-neutral unit or conformance tests for semantics and ACLs. |
| V1 compatibility | Existing endpoint tests plus reference comparison where parity is claimed. |
| V2 contract | Handler integration tests and OpenAPI response/schema assertions. |
| Page dispatch | HTML-vs-machine `Accept`, `HEAD`, mutation bypass, canonical suffix, missing asset, and disabled-feature tests. |
| Frontend | Typecheck, lint, production build, route-level loading/error/empty states, and keyboard/accessibility checks. |
| Product journey | Browser test against assets embedded in the Rust binary, including direct deep links and legacy redirects. |
| Backends | SQLite on every change; RocksDB/Postgres before declaring workflow parity. |

Bundle output is part of the review. Public entry code must not statically import
admin workbench features. Large specialist assets such as Monaco may remain
large only when they are isolated to the route that needs them.

## Change rules

- Add a frontend route and its server page classification in the same change.
- Add or change a V2 response only with its OpenAPI schema and response test.
- Change a V1 route only with an explicit compatibility statement and fixture.
- Introduce privileged UI behavior only with server-side authorization evidence.
- Reuse or extend design-system primitives before adding route-local variants.
- Preserve loading, empty, error, forbidden, and unsupported states as first-
  class designs rather than afterthoughts.
- Keep public prose and imported metadata untrusted; render plain text unless a
  separately reviewed sanitizer and content policy are introduced.
