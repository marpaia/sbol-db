import assert from "node:assert/strict";
import test from "node:test";

import {
  contentFingerprint,
  objectProperties,
  propertyLabel,
} from "./object-presentation.ts";

test("normalizes resources and literals without dropping unknown values", () => {
  const properties = objectProperties({
    "@id": "https://example.org/part",
    "@type": ["http://sbols.org/v3#Component"],
    "http://sbols.org/v3#hasSequence": [
      { "@id": "https://example.org/sequence" },
    ],
    "http://purl.org/dc/terms/created": [
      {
        "@value": "2026-07-30T10:00:00Z",
        "@type": "http://www.w3.org/2001/XMLSchema#dateTime",
      },
    ],
    "https://example.org/extension#evidence": [{ score: 0.9 }],
  });

  assert.equal(properties.length, 3);
  assert.equal(properties[0].label, "Created");
  assert.deepEqual(properties[0].values[0], {
    kind: "literal",
    value: "2026-07-30T10:00:00Z",
    datatype: "http://www.w3.org/2001/XMLSchema#dateTime",
    language: undefined,
  });
  assert.deepEqual(properties[2].values[0], {
    kind: "resource",
    value: "https://example.org/sequence",
  });
  assert.equal(properties[1].values[0].kind, "json");
});

test("turns compact vocabulary names into readable labels", () => {
  assert.equal(
    propertyLabel("http://sbols.org/v3#hasSequence"),
    "Has Sequence"
  );
  assert.equal(propertyLabel("https://example.org/source_uri"), "Source uri");
});

test("formats persisted hash bytes as a stable lowercase fingerprint", () => {
  assert.equal(contentFingerprint([0, 15, 16, 255]), "000f10ff");
  assert.equal(contentFingerprint([]), null);
});
