import type { ImportDocumentFormat } from "@/lib/api";

export interface ImportRemoteDocumentValue {
  url: string;
  format: ImportDocumentFormat;
  namespace: string;
  document_iri: string;
  name: string;
  description: string;
  created_by: string;
}

export const importRemoteDocumentEmpty = (): ImportRemoteDocumentValue => ({
  url: "",
  format: "rdfxml",
  namespace: "",
  document_iri: "",
  name: "",
  description: "",
  created_by: "",
});

export function importRemoteDocumentValidate(
  v: ImportRemoteDocumentValue
): string | null {
  if (!v.url.trim()) return "URL is required.";
  try {
    const parsed = new URL(v.url.trim());
    if (parsed.protocol !== "https:") return "URL must use https.";
  } catch {
    return "URL must be valid.";
  }
  return null;
}

export function importRemoteDocumentToPayload(
  v: ImportRemoteDocumentValue
): unknown {
  const payload: Record<string, unknown> = {
    url: v.url.trim(),
    format: v.format,
  };
  if (v.namespace.trim()) payload.namespace = v.namespace.trim();
  if (v.document_iri.trim()) payload.document_iri = v.document_iri.trim();
  if (v.name.trim()) payload.name = v.name.trim();
  if (v.description.trim()) payload.description = v.description.trim();
  if (v.created_by.trim()) payload.created_by = v.created_by.trim();
  return payload;
}
