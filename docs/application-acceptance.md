# SBOL DB autonomous application acceptance contract

This document is the standing delivery contract for the phased SBOL DB
application roadmap. It turns product intent into executable gates so additive
work can proceed without routine manual validation. A phase is complete only
when its required behavior and evidence are both present.

The contract complements [the application architecture](portal-architecture.md):
that document owns topology and sequencing; this document owns completion.

## Autonomy boundary

The following work is authorized without an intermediate product decision:

- additive V2 resources and browser routes that preserve the documented route
  ownership boundary;
- application-layer behavior, internal refactors, and reversible additive
  migrations within the existing identity and ACL model;
- extensions to the SBOL DB ShadCN, Radix, and Tailwind design system;
- accessibility, responsive, loading, error, empty, forbidden, and unsupported
  states;
- compatibility fixtures, conformance tests, documentation, and deployment
  telemetry that does not capture request payloads or other sensitive data; and
- feature flags and capability-driven degradation that leave existing defaults
  secure.

Work stops for an explicit decision before:

- removing, disabling, or semantically changing a V1 compatibility endpoint;
- changing SBOL identity minting, publication, ownership, or ACL semantics;
- applying a destructive or difficult-to-reverse data migration;
- changing role powers, public-access defaults, or the SBOL DB product identity;
- selecting or purchasing an external mail or infrastructure provider;
- enabling third-party code execution under a new trust model; or
- choosing between materially different scientific interpretations that cannot
  be resolved from SBOL specifications and reference behavior.

## Increment acceptance gates

Every vertical increment must satisfy all applicable gates below.

### Semantics and storage

- Domain behavior is implemented in `sbol-db-app` or a lower neutral layer,
  never independently in React or an HTTP adapter.
- Identity, graph scope, ownership, visibility, provenance, and mutation
  outcomes have positive and forbidden tests.
- Mutations are atomic. A failed request leaves no partial graph, derived
  object, collection membership, index generation, or account state.
- Search maintenance is scheduled after every committed mutation that changes
  discoverable content.
- Unsupported backend behavior is reported through capabilities and a clear
  error; it is never silently dropped.

### API contract

- Product workflows use a native V2 contract with idiomatic verbs and JSON.
- Handler, OpenAPI schema, typed frontend client, and response tests change
  together.
- Paging has deterministic ordering and a stable tie-breaker. Totals describe
  the whole authorized result set and pages contain no duplicates or omissions.
- Errors use the documented V2 envelope and stable status/code pairs.
- Advertised RDF and sequence representations have byte, parse, or semantic
  equivalence tests.
- V2 and compatibility adapters call shared application services rather than
  calling or scraping each other.

### Compatibility

Every migrated workflow records:

- the classic method, path, request encoding, and authentication transport;
- the V2 method, path, schema, and frontend owner;
- ACL, status, header, identity, and representation behavior;
- whether parity is exact, semantically equivalent with a modern wire shape,
  or intentionally divergent; and
- golden fixtures or a reference-server comparison wherever parity is claimed.

Browser dispatch tests must prove that page routes do not steal machine
requests, reserved object suffixes, mutation methods, missing assets, or unknown
extension paths.

### Security and privacy

- Anonymous, member, curator, and administrator behavior is tested at the HTTP
  boundary for every protected workflow.
- Out-of-scope reads are non-disclosing; unauthenticated and unauthorized
  mutations fail with the documented status.
- Cookie-authenticated mutations enforce the same-origin CSRF policy.
- Bootstrap and frontend payloads contain no credential, token, password, mail
  secret, plugin secret, or unrelated administrator configuration.
- User-authored prose and imported metadata render as untrusted text.
- Destructive administrator actions name their exact target, require deliberate
  confirmation, and produce an audit record.

### Interface and design system

- A route reuses or extends `components/ui` and shared surface components before
  adding a route-local pattern.
- Product colors come from semantic tokens. Deployment metadata never recolors
  or renames the native application.
- Loading, stale, empty, error, forbidden, unsupported, success, and destructive
  confirmation states are implemented where applicable.
- The primary journey can be completed with a keyboard, focus is visible, and
  focus returns predictably when overlays close.
- Automated accessibility checks report no serious or critical violations.
- Reference widths of 320, 768, 1024, and 1440 pixels have no unintended
  horizontal overflow. Primary touch targets are at least 44 by 44 pixels where
  the layout permits.
- Light, dark, reduced-motion, and dense-content states receive rendered review.
- All shareable filters, sorting, and paging survive reload, back/forward, and a
  copied URL.
- Frequent and keyboard-triggered actions are immediate. Functional UI motion
  is property-specific, normally limited to transform and opacity, completes in
  at most 300 ms, and honors reduced motion.
- Popovers originate from their trigger; viewport modals remain centered.
- Unsupported biology is described explicitly rather than omitted.

### Engineering and operations

- Rust formatting, relevant checks, unit tests, integration tests, and backend
  conformance pass.
- TypeScript formatting, typecheck, lint, and production build pass. New
  production files introduce no warnings and the repository warning baseline
  does not increase.
- OpenAPI documents and checked-in fixtures parse and validate.
- `git diff --check` passes.
- Public entry chunks do not import Monaco or administrator-only modules. A
  public bundle increase above 10 percent is investigated and justified.
- At least one journey runs against assets embedded in the Rust binary rather
  than only through Vite.
- Documentation states current limitations and feature-flag behavior.

## Evidence tiers

Every increment records the evaluated commit, acceptance scenarios, exact test
commands, compatibility classification, and known limitations.

| Tier | When it runs | Required evidence |
| --- | --- | --- |
| Change | Every vertical increment | Focused unit/integration tests, SQLite, OpenAPI, typecheck, lint, build, diff check. |
| Phase | Before declaring a roadmap phase complete | Postgres and RocksDB conformance, embedded-binary browser journeys, accessibility scan, responsive light/dark screenshots, bundle comparison. |
| Release | Before a compatibility or deployment claim | Container-shaped startup, migration/conformance fixtures, full available corpus, reference comparison where parity is claimed. |

Small fixtures remain smoke tests. Discovery and compatibility claims use the
complete available corpus or endpoint inventory appropriate to the claim.

## Phase exit contracts

### Phase 1: discovery parity

- Normalized V2 discovery supports text, SBOL class, role, collection, owner,
  provenance, created/modified ranges, sorting, and stable paging.
- Type and role controls are ontology-backed while retaining an exact-IRI
  advanced control.
- Classic path-style links have a documented and tested translation.
- Text, structured, sequence, and similarity discovery have coherent public
  entry points.
- UI and API totals agree; the complete corpus is reachable with no paging
  omission; relevance gates pass without unexplained regression.
- During active roadmap development, the exhaustive SBOLTestSuite gate runs
  with the ranked-text topology by default so corpus, paging, facet, and text
  relevance failures remain fast to diagnose. BGE vector rebuild and semantic
  ranking remain an explicit opt-in release gate; they are not claimed by a
  text-only phase run.

### Phase 2: object understanding

- Canonical identities render the modern page while every reserved machine
  suffix remains an API.
- Identity, type, role, version, provenance, ownership, visibility, citations,
  source graph, sequence, features, interactions, collections, attachments,
  uses, twins, and similarity have reusable representations.
- Missing, empty, partial, and unsupported content are distinguishable.
- Every advertised SBOL, non-recursive SBOL, GenBank, FASTA, GFF3, and OMEX
  representation is verified.
- SBOL Visual has supported, partial, unsupported, and metadata-first fallback
  states.

### Phase 3: contribution and collections

- Members have an account workspace and validate SBOL 2, SBOL 3, GenBank, and
  FASTA before persistence.
- Users see import, identity, warning, and conflict consequences before commit;
  cancellation and failure write nothing.
- Collection creation, editing, membership, ownership, publishing, and removal
  use application commands and V2 contracts.
- A create, inspect, revise, publish, and download journey proves provenance and
  ACL behavior at every transition.

### Phase 4: accounts and collaboration

- Profile and affiliation edits, password change/reset, ownership transfer,
  sharing, and curator review have positive, forbidden, and audit evidence.
- Reset and notification controls remain capability-gated until mail is
  configured and never expose tokens to frontend JavaScript or logs.
- Anonymous, member, curator, administrator, owner, recipient, and revoked-user
  journeys cover the complete role matrix.

### Phase 5: admin control plane

- Every privileged endpoint is inventoried and covered by one administrator
  policy; hiding navigation is never the security boundary.
- Instance configuration, users, remotes, plugins, search indexes, jobs,
  ontologies, backup/restore, and maintenance form coherent capability-aware
  sections.
- Read-only status and mutations are structurally separated; destructive
  operations have confirmation and audit evidence.
- Backup/restore passes an integrity-checked round trip.

### Phase 6: compatibility cutover preparation

- The maintained V1 suite and browser/API collision matrix pass on SQLite,
  RocksDB, and Postgres.
- Representative data and configuration migration is rehearsed.
- V1 and legacy bookmark usage can be measured without sensitive payloads.
- A compatibility table and migration guide identify supported, deprecated,
  intentionally different, and unsupported behavior.
- Every proposed removal has usage evidence, a replacement, notice, and a
  rollback plan. Actual removal remains an explicit decision.

## Current implementation record

Phases 1 through 6 are functionally implemented in the current working tree.
This is an implementation and automated-contract record, not a release commit;
publication must attach the resulting commit identifier. No compatibility route
has been removed or assigned a retirement date.

| Phase | Implemented evidence |
| --- | --- |
| 1 — discovery | Normalized V2 filters/facets, classic-link translation, stable URL paging, separate text/structured/sequence/similarity entry points, and the ranked-text SBOLTestSuite sweep over 447 documents and 8,805 objects. BGE is not claimed by this development gate. |
| 2 — objects | ACL-scoped details service, reusable identity/biology/provenance/relation/attachment/raw-property surfaces, explicit visual fallback states, canonical page routing, and parser/semantic checks for every advertised representation. |
| 3 — contribution | Validate-first SBOL 2, SBOL 3, GenBank, and FASTA workflow; member workspace; collection lifecycle commands; and create/revise/publish/download ACL and provenance tests. |
| 4 — collaboration | Self-scoped account management, password change, capability-gated reset, read-only shares and revocation, distinct ownership transfer, curator review, and append-only activity with the complete role matrix. |
| 5 — administration | One native `/api/v2/admin/*` authorization boundary, typed admin UI, instance/user/integration/search/job/ontology/backup/audit sections, recursive secret redaction, exact confirmations, and checksum-verified atomic registry restore. |
| 6 — cutover | Checked 109-path primary plus 61-alias V1 inventory and two protocol paths; bounded telemetry; three-backend V1, collision, and migration rehearsals; migration/rollback guide; and a clean live classic-reference read differential against Postgres, SQLite, and RocksDB. |

The consolidated non-browser gate is reproducible with these commands (the
Postgres variables must name isolated empty test databases):

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test -p sbol-db-server \
  --test docs_openapi_test --test portal_serving_test \
  --test synbiohub_download_test --test v2_admin_test \
  --test v2_discovery_test --test v2_openapi_test \
  --test v2_parity_test --test v2_resources_test \
  --test v2_session_instance_test
SBOL_DB_MIGRATION_TEST_POSTGRES_URL=postgres://... \
  cargo test -p sbol-db migrates_a_synthetic_mini_dump

cd crates/sbol-db-ui/ui
npm run format:check
npm run typecheck
npm run lint
npm test
npm run build
```

Backend and release-sized gates use the exact commands documented in
[the conformance guide](synbiohub-conformance.md),
[the portal architecture](portal-architecture.md), and
[the built-in search model guide](builtin-bge-small-model.md). The production
UI build includes an executable public-entry guard and fails if Monaco or a
route-only administrator bundle becomes an initial import or preload.

No browser automation was used for this record. As explicitly scoped for this
implementation run, rendered breakpoint, light/dark, reduced-motion, and
browser accessibility review remain manual release-presentation evidence; this
record does not claim those visual checks.
