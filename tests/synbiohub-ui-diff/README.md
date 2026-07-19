# SynBioHub UI differential harness

Drives the real `sbh3frontend` UI against sbol-db, records every request the UI
makes to the backend, then replays each `GET` against both sbol-db and classic
SynBioHub and diffs the **status code and the response body's structural shape**.

It catches the failure the conformance suite and a status-only check miss: a
response that returns `200` but the wrong shape (e.g. `/admin/registries`
returning `{}` where the UI iterates `data.registries`). The UI decides which
endpoints are exercised, so this finds shape bugs on the paths the UI actually
depends on.

## Prerequisites

Three services running:

- the UI at `http://localhost:3333` pointed at sbol-db, and sbol-db at
  `http://localhost:18903` seeded with the conformance corpus. Start both with
  `../synbiohub-ui/run.sh up --seed /tmp/sbol-db-subject.sqlite`.
- classic SynBioHub at `http://localhost:17777` seeded with the same corpus (the
  conformance reference stack).

Both backends must hold the same corpus so object paths resolve on each.

## Run

```
cd tests/synbiohub-ui-diff
npm install          # installs playwright; browsers download once
npm run crawl
```

Overrides: `UI`, `SBOLDB`, `CLASSIC`, `OUT` (report path), `PW_MODULE` (path to
an existing playwright install to reuse its browsers).

Exit code is non-zero when any divergence is found. The full per-endpoint report
is written to `report.json`.

## What it reports

- **DIVERGENCES** — a `GET` whose status or body shape differs between sbol-db
  and classic, excluding auth-scoped ones. These are the actionable bugs.
- **AUTH-SCOPED** — a side answered `401`/`403`; listed separately because the
  two stacks have independent user databases, so a token valid on one is not
  valid on the other. Verify these by hand.
- Mutating requests (`POST`/`PUT`/`DELETE`) are recorded but never replayed
  against classic (they would write to it); the count is printed so the coverage
  gap is explicit.

## Extending

Add UI routes to `ROUTES` in `crawl.cjs`. Object-view routes are filled in from
live sbol-db data automatically. To cover flows behind a click (download modal,
sequence search submit), add the interaction after the `page.goto` loop.
