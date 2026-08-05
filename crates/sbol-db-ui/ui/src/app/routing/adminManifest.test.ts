import assert from "node:assert/strict";
import test from "node:test";

import {
  adminBreadcrumbs,
  adminDestinations,
  availableAdminDestinations,
} from "./adminManifest.ts";

test("admin destinations have unique ids and paths", () => {
  const ids = adminDestinations.map((destination) => destination.id);
  const paths = adminDestinations.map((destination) => destination.path);

  assert.equal(new Set(ids).size, ids.length);
  assert.equal(new Set(paths).size, paths.length);
});

test("capability predicates drive every navigation surface consistently", () => {
  const unavailable = availableAdminDestinations(undefined);
  assert.equal(
    unavailable.some((item) => item.id === "sql"),
    false
  );
  assert.equal(
    unavailable.some((item) => item.id === "maintenance"),
    false
  );

  const available = availableAdminDestinations({
    sql_console: true,
    relational_schema: true,
    maintenance: "relational",
    slow_query_stats: true,
    activity_and_locks: true,
  });
  assert.equal(
    available.some((item) => item.id === "sql"),
    true
  );
  assert.equal(
    available.some((item) => item.id === "maintenance"),
    true
  );
});

test("breadcrumbs use the same destination metadata as navigation", () => {
  assert.deepEqual(adminBreadcrumbs("/admin/graphs/example-graph"), [
    { label: "Data model" },
    { label: "Graphs", to: "/admin/graphs" },
    { label: "example-…", mono: true },
  ]);
  assert.deepEqual(adminBreadcrumbs("/admin/objects/lookup"), [
    { label: "Data model" },
    { label: "Objects", to: "/admin/objects" },
    { label: "Bulk lookup" },
  ]);
});
