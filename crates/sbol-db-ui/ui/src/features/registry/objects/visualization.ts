import type { ObjectVisualFeature, ObjectVisualGlyph } from "./api.ts";

export interface LaidOutVisualFeature {
  feature: ObjectVisualFeature & { start: number; end: number };
  lane: number;
}

export interface VisualSpan {
  x: number;
  width: number;
  exactX: number;
  exactWidth: number;
}

export interface SequenceWindowPart {
  kind: "flank" | "feature" | "ellipsis";
  text: string;
}

export interface SequenceWindow {
  start: number;
  end: number;
  parts: SequenceWindowPart[];
}

export interface SequencePreview {
  head: string;
  tail: string;
  omitted: number;
}

export const visualGlyphLabels: Record<ObjectVisualGlyph, string> = {
  promoter: "Promoter",
  coding_sequence: "Coding sequence",
  ribosome_entry_site: "Ribosome entry site",
  terminator: "Terminator",
  operator: "Operator",
  origin_of_replication: "Origin of replication",
  unspecified: "Unspecified feature",
};

/**
 * Select a component-level glyph only from an exact Sequence Ontology role.
 * This intentionally mirrors the server's feature projection so a leaf
 * Component can be represented without pretending it is its own child feature.
 */
export function visualGlyphForRoles(roles: string[]): ObjectVisualGlyph {
  if (roles.some((role) => roleHasAccession(role, "0000167")))
    return "promoter";
  if (roles.some((role) => roleHasAccession(role, "0000316")))
    return "coding_sequence";
  if (roles.some((role) => roleHasAccession(role, "0000139")))
    return "ribosome_entry_site";
  if (roles.some((role) => roleHasAccession(role, "0000141")))
    return "terminator";
  if (roles.some((role) => roleHasAccession(role, "0000057")))
    return "operator";
  if (roles.some((role) => roleHasAccession(role, "0000296")))
    return "origin_of_replication";
  return "unspecified";
}

/** Keep a sequence preview bounded while retaining evidence from both ends. */
export function sequencePreview(
  elements: string,
  maximumBases = 144
): SequencePreview {
  const maximum = Math.max(2, maximumBases);
  if (elements.length <= maximum)
    return { head: elements, tail: "", omitted: 0 };

  const tailLength = Math.max(1, Math.floor(maximum / 3));
  const headLength = maximum - tailLength;
  return {
    head: elements.slice(0, headLength),
    tail: elements.slice(-tailLength),
    omitted: elements.length - maximum,
  };
}

export function hasVisualRange(
  feature: ObjectVisualFeature
): feature is ObjectVisualFeature & { start: number; end: number } {
  return (
    feature.start !== null &&
    feature.end !== null &&
    feature.start > 0 &&
    feature.start <= feature.end
  );
}

export function visualExtent(
  features: ObjectVisualFeature[],
  assertedSequenceLength: number | null
) {
  return Math.max(
    assertedSequenceLength || 0,
    ...features.map((feature) => feature.end || 0),
    1
  );
}

export function layoutVisualFeatures(
  features: ObjectVisualFeature[]
): LaidOutVisualFeature[] {
  const laneEnds: number[] = [];
  return features
    .filter(hasVisualRange)
    .sort(
      (left, right) =>
        left.start - right.start ||
        left.end - right.end ||
        left.uri.localeCompare(right.uri)
    )
    .map((feature) => {
      let lane = laneEnds.findIndex((end) => end < feature.start);
      if (lane < 0) lane = laneEnds.length;
      laneEnds[lane] = feature.end;
      return { feature, lane };
    });
}

/**
 * Keep single-base and very short features operable without pretending their
 * visible width is their biological span. The exact interval is returned
 * separately and is always drawn as the coordinate truth.
 */
export function visualSpan(
  start: number,
  end: number,
  maximum: number,
  trackWidth: number,
  minimumWidth = 22
): VisualSpan {
  const exactX = ((start - 1) / maximum) * trackWidth;
  const exactWidth = ((end - start + 1) / maximum) * trackWidth;
  const width = Math.min(trackWidth, Math.max(minimumWidth, exactWidth));
  const center = exactX + exactWidth / 2;
  const x = Math.max(0, Math.min(trackWidth - width, center - width / 2));
  return { x, width, exactX, exactWidth };
}

export function visualFeatureLength(feature: ObjectVisualFeature) {
  return hasVisualRange(feature) ? feature.end - feature.start + 1 : null;
}

export function isReverseOrientation(orientation: string | null) {
  if (!orientation) return false;
  const compact = orientation.replace(/[#/]/g, "");
  return (
    /reverse(?:Complement|_complement)?$/i.test(compact) ||
    /SO[:_]0001031$/i.test(orientation)
  );
}

export function orientationLabel(orientation: string | null) {
  if (!orientation) return "Orientation not asserted";
  if (isReverseOrientation(orientation)) return "Reverse complement";
  if (
    /inline$/i.test(orientation.replace(/[#/]/g, "")) ||
    /SO[:_]0001030$/i.test(orientation)
  )
    return "Forward";
  return iriLabel(orientation);
}

export function featureSequenceWindow(
  elements: string,
  feature: ObjectVisualFeature,
  flankLength = 12,
  maximumFeatureBases = 48
): SequenceWindow | null {
  if (!hasVisualRange(feature) || elements.length === 0) return null;
  const featureStart = Math.min(elements.length, feature.start - 1);
  const featureEnd = Math.min(elements.length, feature.end);
  if (featureStart >= featureEnd) return null;

  const selectedLength = featureEnd - featureStart;
  if (selectedLength <= maximumFeatureBases) {
    const windowStart = Math.max(0, featureStart - flankLength);
    const windowEnd = Math.min(elements.length, featureEnd + flankLength);
    return {
      start: windowStart + 1,
      end: windowEnd,
      parts: [
        { kind: "flank", text: elements.slice(windowStart, featureStart) },
        { kind: "feature", text: elements.slice(featureStart, featureEnd) },
        { kind: "flank", text: elements.slice(featureEnd, windowEnd) },
      ].filter((part) => part.text.length > 0) as SequenceWindowPart[],
    };
  }

  const half = Math.max(1, Math.floor(maximumFeatureBases / 2));
  return {
    start: featureStart + 1,
    end: featureEnd,
    parts: [
      {
        kind: "feature",
        text: elements.slice(featureStart, featureStart + half),
      },
      { kind: "ellipsis", text: "…" },
      { kind: "feature", text: elements.slice(featureEnd - half, featureEnd) },
    ],
  };
}

export function compactRole(role: string) {
  const obo = role.match(/(?:[/#]|_)(SO)[_:](\d+)$/i);
  if (obo) return `${obo[1].toUpperCase()}:${obo[2]}`;
  const identifiers = role.match(/\b(SO):(\d+)$/i);
  if (identifiers) return `${identifiers[1].toUpperCase()}:${identifiers[2]}`;
  return iriLabel(role);
}

function iriLabel(iri: string) {
  return iri.split(/[/#]/).filter(Boolean).at(-1)?.replaceAll("_", " ") || iri;
}

function roleHasAccession(role: string, accession: string) {
  const tail = role.split(/[/#]/).filter(Boolean).at(-1);
  return (
    tail?.toUpperCase() === `SO:${accession}` ||
    tail?.toUpperCase() === `SO_${accession}`
  );
}
