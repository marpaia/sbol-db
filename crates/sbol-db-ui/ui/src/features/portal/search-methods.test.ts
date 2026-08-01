import assert from "node:assert/strict";
import test from "node:test";

import type { SearchStrategiesResponse } from "./api.ts";
import {
  activeSearchMethod,
  buildSearchMethods,
  paramsForSearchMethod,
} from "./search-methods.ts";

const strategies: SearchStrategiesResponse = {
  default_strategy: "semantic.v2",
  items: [
    {
      id: "semantic.v2",
      version: "2",
      display_name: "Contextual vectors",
      description: "Semantic SBOL search",
      capabilities: {
        inputs: ["text"],
        filters: ["graph"],
        filter_execution: "native",
        pagination: "cursor",
        totals: "unknown",
        deterministic: false,
        explanations: true,
        data_egress: "none",
      },
      requirements: { vector_indexes: ["sbol-v2"] },
    },
    {
      id: "multi.v1",
      version: "1",
      display_name: "Multimodal",
      description: "Text and sequence search",
      capabilities: {
        inputs: ["text", "sequence"],
        filters: ["object_type"],
        filter_execution: "post_filter",
        pagination: "first_page_only",
        totals: "lower_bound",
        deterministic: true,
        explanations: false,
        data_egress: "none",
      },
      requirements: {},
    },
  ],
};

test("builds public methods for every configured strategy input", () => {
  const methods = buildSearchMethods(strategies);

  assert.deepEqual(
    methods.map((method) => method.key),
    [
      "native",
      "structured:semantic.v2:text",
      "structured:multi.v1:text",
      "structured:multi.v1:sequence",
      "sequence",
    ]
  );
  assert.equal(methods[1].label, "Contextual vectors");
  assert.equal(methods[3].label, "Multimodal · DNA sequence");
});

test("selects a structured strategy from canonical URL state", () => {
  const methods = buildSearchMethods(strategies);
  const selected = activeSearchMethod(
    new URLSearchParams("strategy=multi.v1&kind=sequence"),
    methods
  );

  assert.equal(selected.key, "structured:multi.v1:sequence");
});

test("switches methods without carrying incompatible filters", () => {
  const methods = buildSearchMethods(strategies);
  const semantic = methods.find(
    (method) => method.key === "structured:semantic.v2:text"
  );
  assert.ok(semantic);
  const next = paramsForSearchMethod(
    new URLSearchParams(
      "q=promoter&type=Component&role=promoter&offset=24&limit=48"
    ),
    semantic
  );

  assert.equal(next.get("q"), "promoter");
  assert.equal(next.get("strategy"), "semantic.v2");
  assert.equal(next.get("kind"), "text");
  assert.equal(next.has("type"), false);
  assert.equal(next.has("role"), false);
  assert.equal(next.has("offset"), false);
  assert.equal(next.has("limit"), false);
});
