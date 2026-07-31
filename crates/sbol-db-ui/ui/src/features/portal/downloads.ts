import type { PortalObjectDetails } from "./api";

export interface SequenceDownloadAvailability {
  state: "available" | "unavailable" | "unknown";
  note: string | null;
}

/**
 * Decide only what the normalized object resource proves. A Sequence with
 * elements is exportable; a residue-free Sequence is not. A metadata-only
 * Component with neither sequences nor outward feature structure cannot lead
 * to a sequence record. Richer recursive closures remain unknown and are
 * allowed through to the server, which is the final export authority.
 */
export function sequenceDownloadAvailability(
  object: PortalObjectDetails
): SequenceDownloadAvailability {
  if (isObjectType(object.object_type, "Sequence")) {
    const available = (object.sequence_content.length ?? 0) > 0;
    return available
      ? { state: "available", note: null }
      : {
          state: "unavailable",
          note: "No sequence elements are stored for this object.",
        };
  }

  if (
    isObjectType(object.object_type, "Component") ||
    isObjectType(object.object_type, "ComponentDefinition")
  ) {
    if (
      object.sequences.state === "empty" &&
      object.features.state === "empty"
    ) {
      return {
        state: "unavailable",
        note: "No sequence elements are stored for this object.",
      };
    }
    return { state: "unknown", note: null };
  }

  return { state: "unknown", note: null };
}

function isObjectType(iri: string, localName: string): boolean {
  return iri.endsWith(`#${localName}`) || iri.endsWith(`/${localName}`);
}
