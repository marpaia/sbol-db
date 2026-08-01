export function shortIri(iri?: string | null): string {
  if (!iri) return "SBOL object";
  const match = iri.match(/[#/]([^#/]+)$/);
  return match?.[1] || iri;
}
