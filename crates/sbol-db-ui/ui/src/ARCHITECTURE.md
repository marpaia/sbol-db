# Frontend architecture

The frontend is organized around application composition, user-facing features,
and dependency-light shared code.

## Layers

```text
main.tsx
  app/                 providers, route composition, route metadata
    routing/
    providers/
  features/            behavior owned by a product domain
    instance/
    session/
    registry/
      discovery/
      objects/
      contributions/
      collaboration/
      account/
      workspace/
    admin/
      overview/
      workbench/
      schema/
      graphs/
      imports/
      objects/
      neighborhood/
      sequences/
      ontologies/
      observability/
      jobs/
      maintenance/
      settings/
  components/          reusable presentation within a product surface
    ui/                 design-system primitives
    product/            cross-surface product chrome
    portal/             registry presentation
    lab/                admin workspace presentation
  shared/               framework helpers with no feature knowledge
  routes/               only cross-feature gates and static information pages
```

Dependencies point downward: `app` composes features; features may use
components and shared utilities; shared code never imports from app, routes, or
features. Components and route shells consume a feature's public `api.ts` and
`queries.ts`, not the historical aggregate API modules.

## Routing

`App.tsx` is the public/admin bundle boundary. `PublicRoutes.tsx` owns the
registry route tree and `AdminApp.tsx` owns the lazily loaded administrator tree.
The administrator destination manifest is the source of truth for route labels,
paths, icons, capability requirements, navigation, the command palette, and
breadcrumbs. A new static administrator destination should be added to the
manifest before it is wired into `AdminApp.tsx`.

Feature route components live next to their API and query code. The root
`routes/` directory is reserved for gates and truly cross-feature or static
pages.

## Server state and requests

Every domain defines a query-key factory in its `queries.ts`. Mutations
invalidate those factories rather than repeating array literals. Cross-feature
invalidation imports the other feature's key factory explicitly so the
dependency is visible.

`shared/api/http.ts` owns HTTP execution and normalized errors. Feature APIs own
URLs, request/response types, and domain operations. `features/registry/api.ts`,
`features/admin/api.ts`, and `lib/api.ts` remain private endpoint catalogs while
the server contract is incrementally generated; only feature-level `api.ts`
facades may import them. UI code must use the owning feature facade.

## Client state

The admin query workbench store lives under `features/admin/workbench`. Its
persisted storage key is intentionally stable for existing users. Generic hooks
such as copy-to-clipboard, mobile detection, and command shortcuts live under
`shared/hooks`.

## Tests and boundaries

Pure domain transformations are tested next to the feature. Routing metadata and
shared transport behavior have focused tests. ESLint prevents shared code from
depending upward and prevents presentation code from bypassing feature API
facades. The required local gate is:

```sh
npm run format:check
npm run typecheck
npm run lint
npm test
npm run build
```
