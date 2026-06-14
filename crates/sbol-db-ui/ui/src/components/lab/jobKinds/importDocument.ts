/**
 * Value shape, empty/default constructor, validator, and JSON-payload
 * serializer for the `import_document` job. Mirrors `ImportDocumentPayload`
 * in `crates/sbol-db-jobs/src/handlers/import_document.rs`.
 *
 * Lives next to the form component (`ImportDocumentForm.tsx`) but in its
 * own non-JSX module so the component file can be HMR-friendly.
 */

import type { ImportDocumentFormat } from "@/lib/api";

export interface ImportDocumentValue {
  body: string;
  format: ImportDocumentFormat;
  namespace: string;
  source_uri: string;
  document_iri: string;
  name: string;
  description: string;
  created_by: string;
}

export const importDocumentEmpty = (): ImportDocumentValue => ({
  body: "",
  format: "turtle",
  namespace: "",
  source_uri: "",
  document_iri: "",
  name: "",
  description: "",
  created_by: "",
});

export function importDocumentValidate(v: ImportDocumentValue): string | null {
  if (!v.body.trim()) return "Document body is required.";
  return null;
}

/**
 * Build the JSON payload the server expects. Empty optional fields are
 * omitted so the server's `#[serde(default)]` defaults apply.
 */
export function importDocumentToPayload(v: ImportDocumentValue): unknown {
  const payload: Record<string, unknown> = {
    body: v.body,
    format: v.format,
  };
  if (v.namespace.trim()) payload.namespace = v.namespace.trim();
  if (v.source_uri.trim()) payload.source_uri = v.source_uri.trim();
  if (v.document_iri.trim()) payload.document_iri = v.document_iri.trim();
  if (v.name.trim()) payload.name = v.name.trim();
  if (v.description.trim()) payload.description = v.description.trim();
  if (v.created_by.trim()) payload.created_by = v.created_by.trim();
  return payload;
}
