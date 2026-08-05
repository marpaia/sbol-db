import assert from "node:assert/strict";
import test from "node:test";

import type { ObjectVisualFeature } from "./api.ts";
import {
  compactRole,
  featureSequenceWindow,
  layoutVisualFeatures,
  orientationLabel,
  sequencePreview,
  visualExtent,
  visualGlyphForRoles,
  visualSpan,
} from "./visualization.ts";

function feature(
  uri: string,
  start: number | null,
  end: number | null
): ObjectVisualFeature {
  return {
    uri,
    label: uri,
    roles: [],
    glyph: "unspecified",
    start,
    end,
    orientation: null,
  };
}

test("lays overlapping features into deterministic lanes", () => {
  const layout = layoutVisualFeatures([
    feature("late", 21, 30),
    feature("wide", 1, 20),
    feature("overlap", 10, 12),
    feature("unplaced", null, null),
  ]);

  assert.deepEqual(
    layout.map(({ feature: item, lane }) => [item.uri, lane]),
    [
      ["wide", 0],
      ["overlap", 1],
      ["late", 0],
    ]
  );
});

test("extracts bounded sequence evidence around short and long features", () => {
  assert.deepEqual(
    featureSequenceWindow("AACCGGTTAACC", feature("short", 5, 8), 2, 6),
    {
      start: 3,
      end: 10,
      parts: [
        { kind: "flank", text: "CC" },
        { kind: "feature", text: "GGTT" },
        { kind: "flank", text: "AA" },
      ],
    }
  );
  assert.deepEqual(
    featureSequenceWindow("AACCGGTTAACC", feature("long", 2, 11), 2, 6),
    {
      start: 2,
      end: 11,
      parts: [
        { kind: "feature", text: "ACC" },
        { kind: "ellipsis", text: "…" },
        { kind: "feature", text: "AAC" },
      ],
    }
  );
});

test("keeps exact coordinate spans separate from minimum hit areas", () => {
  const span = visualSpan(100, 100, 1_000, 900);
  assert.ok(Math.abs(span.exactX - 89.1) < Number.EPSILON * 100);
  assert.ok(Math.abs(span.exactWidth - 0.9) < Number.EPSILON * 10);
  assert.equal(span.width, 22);
  assert.ok(Math.abs(span.x - 78.55) < Number.EPSILON * 100);
});

test("uses asserted sequence length without clipping longer feature ranges", () => {
  assert.equal(visualExtent([feature("a", 1, 120)], 100), 120);
  assert.equal(visualExtent([], null), 1);
});

test("formats standard roles and orientations for the feature inspector", () => {
  assert.equal(
    compactRole("http://identifiers.org/so/SO:0000316"),
    "SO:0000316"
  );
  assert.equal(
    compactRole("http://purl.obolibrary.org/obo/SO_0000167"),
    "SO:0000167"
  );
  assert.equal(
    orientationLabel("http://sbols.org/v2#reverseComplement"),
    "Reverse complement"
  );
  assert.equal(orientationLabel("http://sbols.org/v3#inline"), "Forward");
  assert.equal(
    orientationLabel("https://identifiers.org/SO:0001030"),
    "Forward"
  );
  assert.equal(
    orientationLabel("https://identifiers.org/SO:0001031"),
    "Reverse complement"
  );
});

test("selects component glyphs only from exact ontology accessions", () => {
  assert.equal(
    visualGlyphForRoles(["http://identifiers.org/so/SO:0000316"]),
    "coding_sequence"
  );
  assert.equal(
    visualGlyphForRoles(["http://purl.obolibrary.org/obo/SO_0000167"]),
    "promoter"
  );
  assert.equal(
    visualGlyphForRoles(["http://example.org/SO:00003160"]),
    "unspecified"
  );
});

test("bounds component sequence previews while retaining both ends", () => {
  assert.deepEqual(sequencePreview("AACCGG", 8), {
    head: "AACCGG",
    tail: "",
    omitted: 0,
  });
  assert.deepEqual(sequencePreview("AACCGGTTAACC", 6), {
    head: "AACC",
    tail: "CC",
    omitted: 6,
  });
});
