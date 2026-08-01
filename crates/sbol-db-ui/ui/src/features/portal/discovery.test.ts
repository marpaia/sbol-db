import assert from "node:assert/strict";
import test from "node:test";

import {
  parseDiscoveryParams,
  translateClassicSearchPath,
} from "./discovery.ts";

test("parses every native discovery parameter into typed URL state", () => {
  const params = new URLSearchParams({
    q: "promoter",
    type: "http://sbols.org/v3#Component",
    role: "http://identifiers.org/so/SO:0000167",
    collection: "https://example.org/collections/core",
    owner: "https://example.org/user/alice",
    provenance: "iGEM",
    created_after: "2020-01-01",
    created_before: "2026-01-01",
    modified_after: "2021-01-01",
    modified_before: "2025-01-01",
    sort: "name",
    direction: "desc",
    offset: "24",
    limit: "48",
    view: "list",
  });

  const state = parseDiscoveryParams(params);
  assert.deepEqual(state.query, {
    q: "promoter",
    type: "http://sbols.org/v3#Component",
    role: "http://identifiers.org/so/SO:0000167",
    collection: "https://example.org/collections/core",
    owner: "https://example.org/user/alice",
    provenance: "iGEM",
    createdAfter: "2020-01-01",
    createdBefore: "2026-01-01",
    modifiedAfter: "2021-01-01",
    modifiedBefore: "2025-01-01",
    sort: "name",
    direction: "desc",
    offset: 24,
    limit: 48,
  });
  assert.equal(state.view, "list");
});

test("normalizes invalid presentation state without sending malformed API values", () => {
  const state = parseDiscoveryParams(
    new URLSearchParams(
      "sort=unknown&direction=sideways&offset=-2&limit=0&created_after=yesterday"
    )
  );

  assert.equal(state.query.sort, "relevance");
  assert.equal(state.query.direction, "desc");
  assert.equal(state.query.offset, 0);
  assert.equal(state.query.limit, 24);
  assert.equal(state.query.createdAfter, undefined);
});

test("translates supported classic facets into a canonical native query", () => {
  const translation = translateClassicSearchPath(
    "objectType=ComponentDefinition&sbol2:role=http://identifiers.org/so/SO:0000167&collection=https://example.org/c&createdAfter=2020-01-01&promoter"
  );
  const translated = translation.params;

  assert.equal(translation.pathname, "/search");
  assert.equal(translated.get("compat"), "classic");
  assert.equal(
    translated.get("type"),
    "http://sbols.org/v2#ComponentDefinition"
  );
  assert.equal(translated.get("role"), "http://identifiers.org/so/SO:0000167");
  assert.equal(translated.get("collection"), "https://example.org/c");
  assert.equal(translated.get("created_after"), "2020-01-01");
  assert.equal(translated.get("q"), "promoter");
  assert.deepEqual(translated.getAll("compat_warning"), []);
});

test("keeps every lossy metadata translation visible", () => {
  const translated = translateClassicSearchPath("displayId=BBa_J23100").params;
  const warnings = translated.getAll("compat_warning");

  assert.equal(warnings.length, 1);
  assert.match(warnings[0], /displayId/);
  assert.equal(translated.has("q"), false);
});

test("sends classic sequence grammar to the dedicated public workflow", () => {
  const translation = translateClassicSearchPath(
    "objectType=Sequence&exactsequence=ATGC"
  );

  assert.equal(translation.pathname, "/sequence-search");
  assert.equal(translation.params.get("q"), "ATGC");
  assert.equal(translation.params.get("mode"), "exact");
  assert.match(
    translation.params.get("compat_warning") || "",
    /ignores the additional segment/
  );
});

test("expands SBOL 3 object types and rejects unknown CURIE prefixes", () => {
  const sbol3 = translateClassicSearchPath("objectType=sbol3:Component").params;
  assert.equal(sbol3.get("type"), "http://sbols.org/v3#Component");

  const unknown = translateClassicSearchPath(
    "objectType=example:Component"
  ).params;
  assert.equal(unknown.has("type"), false);
  assert.match(unknown.get("compat_warning") || "", /unknown prefix/);
});
