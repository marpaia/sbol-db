export type ObjectPropertyValue =
  | { kind: "resource"; value: string }
  | {
      kind: "literal";
      value: string;
      datatype?: string;
      language?: string;
    }
  | { kind: "json"; value: unknown };

export interface ObjectProperty {
  iri: string;
  label: string;
  values: ObjectPropertyValue[];
  resourceCount: number;
  literalCount: number;
}

const JSON_LD_KEYS = new Set(["@id", "@type"]);

/**
 * Normalize the deterministic per-object RDF property bag into a stable,
 * presentation-only list. Biological interpretation remains a server concern;
 * unknown terms are retained verbatim instead of guessed or dropped.
 */
export function objectProperties(
  data: Record<string, unknown>
): ObjectProperty[] {
  return Object.entries(data)
    .filter(([iri]) => !JSON_LD_KEYS.has(iri))
    .map(([iri, rawValues]) => {
      const values = (Array.isArray(rawValues) ? rawValues : [rawValues]).map(
        objectPropertyValue
      );
      return {
        iri,
        label: propertyLabel(iri),
        resourceCount: values.filter((value) => value.kind === "resource")
          .length,
        literalCount: values.filter((value) => value.kind === "literal").length,
        values,
      };
    })
    .sort(
      (left, right) =>
        left.label.localeCompare(right.label, undefined, {
          sensitivity: "base",
        }) || left.iri.localeCompare(right.iri)
    );
}

export function propertyLabel(iri: string): string {
  const compact = iri.match(/[#/]([^#/]+)$/)?.[1] || iri;
  const spaced = compact
    .replace(/[_-]+/g, " ")
    .replace(/([a-z\d])([A-Z])/g, "$1 $2")
    .trim();
  return spaced ? spaced.charAt(0).toUpperCase() + spaced.slice(1) : "Property";
}

export function contentFingerprint(bytes: number[]): string | null {
  if (bytes.length === 0) return null;
  return bytes
    .map((byte) =>
      Math.max(0, Math.min(255, byte)).toString(16).padStart(2, "0")
    )
    .join("");
}

function objectPropertyValue(value: unknown): ObjectPropertyValue {
  if (isRecord(value)) {
    if (typeof value["@id"] === "string") {
      return { kind: "resource", value: value["@id"] };
    }
    if (typeof value["@value"] === "string") {
      return {
        kind: "literal",
        value: value["@value"],
        datatype:
          typeof value["@type"] === "string" ? value["@type"] : undefined,
        language:
          typeof value["@language"] === "string"
            ? value["@language"]
            : undefined,
      };
    }
  }
  if (typeof value === "string" || typeof value === "number") {
    return { kind: "literal", value: String(value) };
  }
  return { kind: "json", value };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
