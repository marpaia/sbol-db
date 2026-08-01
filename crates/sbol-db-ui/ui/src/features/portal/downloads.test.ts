import assert from "node:assert/strict";
import test from "node:test";

import type { PortalObjectDetails } from "./api.ts";
import { sequenceDownloadAvailability } from "./downloads.ts";

function details(overrides: Partial<PortalObjectDetails>): PortalObjectDetails {
  return {
    iri: "https://example.org/object",
    persistent_identity: null,
    display_id: "object",
    version: null,
    name: "Object",
    description: null,
    object_type: "http://sbols.org/v3#Component",
    types: [],
    roles: [],
    source_graph: "http://synbiohub.org/public",
    visibility: "public",
    owners: [],
    created_at: null,
    modified_at: null,
    provenance: {
      creators: [],
      derived_from: [],
      generated_by: [],
      mutable_source: [],
      citations: [],
    },
    sequence_content: {
      state: "unsupported",
      elements: null,
      encoding: null,
      length: null,
      note: null,
    },
    sequences: { state: "empty", items: [], note: null },
    features: { state: "empty", items: [], note: null },
    visualization: {
      state: "empty",
      sequence_length: null,
      features: [],
      note: null,
    },
    interactions: { state: "empty", items: [], note: null },
    collections: { state: "empty", items: [], note: null },
    members: { state: "unsupported", items: [], note: null },
    attachments: { state: "empty", items: [], note: null },
    uses: { state: "empty", items: [], note: null },
    twins: { state: "unsupported", items: [], note: null },
    properties: [],
    content_fingerprint: null,
    ...overrides,
  };
}

test("marks a metadata-only Component as unavailable for sequence downloads", () => {
  assert.deepEqual(sequenceDownloadAvailability(details({})), {
    state: "unavailable",
    note: "No sequence elements are stored for this object.",
  });
});

test("marks a standalone Sequence with elements as available", () => {
  const object = details({
    object_type: "http://sbols.org/v3#Sequence",
    sequence_content: {
      state: "available",
      elements: "atgc",
      encoding: "https://identifiers.org/edam:format_1207",
      length: 4,
      note: null,
    },
  });
  assert.deepEqual(sequenceDownloadAvailability(object), {
    state: "available",
    note: null,
  });
});

test("leaves recursive closures for the server to decide", () => {
  const collection = details({
    object_type: "http://sbols.org/v3#Collection",
  });
  assert.deepEqual(sequenceDownloadAvailability(collection), {
    state: "unknown",
    note: null,
  });
});
