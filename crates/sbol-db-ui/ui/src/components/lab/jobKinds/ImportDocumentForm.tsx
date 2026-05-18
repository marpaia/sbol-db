/**
 * Structured form for the `import_document` job. Mirrors
 * `ImportDocumentPayload` in `crates/sbol-db-jobs/src/handlers/import_document.rs`.
 *
 * Use this when ingesting a large document where you want a job id to
 * poll, retries on transient DB failures, and visibility into per-file
 * progress. The synchronous `POST /documents` flow remains the right
 * surface for small one-shot imports — that path is the
 * `DocumentImportDialog` reachable from the Documents page.
 */

import {
  SERIALIZATION_FORMATS,
  serializationLabel,
  type SerializationFormat,
} from "@/lib/api";

export interface ImportDocumentValue {
  body: string;
  format: SerializationFormat;
  source_uri: string;
  document_iri: string;
  name: string;
  description: string;
  created_by: string;
}

export const importDocumentEmpty = (): ImportDocumentValue => ({
  body: "",
  format: "turtle",
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
 * Build the JSON payload the server expects. Empty optional fields
 * are omitted so the server's `#[serde(default)]` defaults apply.
 */
export function importDocumentToPayload(v: ImportDocumentValue): unknown {
  const payload: Record<string, unknown> = {
    body: v.body,
    format: v.format,
  };
  if (v.source_uri.trim()) payload.source_uri = v.source_uri.trim();
  if (v.document_iri.trim()) payload.document_iri = v.document_iri.trim();
  if (v.name.trim()) payload.name = v.name.trim();
  if (v.description.trim()) payload.description = v.description.trim();
  if (v.created_by.trim()) payload.created_by = v.created_by.trim();
  return payload;
}

export interface ImportDocumentFormProps {
  value: ImportDocumentValue;
  onChange: (v: ImportDocumentValue) => void;
  disabled?: boolean;
}

export function ImportDocumentForm({
  value,
  onChange,
  disabled,
}: ImportDocumentFormProps) {
  const patch = (delta: Partial<ImportDocumentValue>) =>
    onChange({ ...value, ...delta });

  return (
    <div className="space-y-3">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-[1fr,160px]">
        <TextInput
          label="Name"
          value={value.name}
          onChange={(s) => patch({ name: s })}
          disabled={disabled}
          placeholder="(optional display name)"
        />
        <FormatSelect
          value={value.format}
          onChange={(s) => patch({ format: s })}
          disabled={disabled}
        />
      </div>

      <label className="block">
        <span className="mb-1.5 block text-sm font-medium text-foreground">
          Body
          <span className="ml-1 text-destructive">*</span>
        </span>
        <textarea
          value={value.body}
          onChange={(e) => patch({ body: e.target.value })}
          disabled={disabled}
          spellCheck={false}
          rows={10}
          placeholder="Paste the document body in the selected format."
          className="w-full resize-y rounded-md border bg-background px-3 py-2 font-mono text-xs text-foreground outline-none placeholder:text-muted-foreground/60 focus:ring-1 focus:ring-ring disabled:opacity-50"
        />
      </label>

      <details className="rounded-md border bg-card">
        <summary className="cursor-pointer select-none px-3 py-2 text-xs font-medium uppercase tracking-wider text-muted-foreground">
          Optional metadata
        </summary>
        <div className="grid gap-3 border-t px-3 py-3 sm:grid-cols-2">
          <TextInput
            label="Document IRI"
            value={value.document_iri}
            onChange={(s) => patch({ document_iri: s })}
            disabled={disabled}
            placeholder="https://example.org/doc/1"
          />
          <TextInput
            label="Source URI"
            value={value.source_uri}
            onChange={(s) => patch({ source_uri: s })}
            disabled={disabled}
            placeholder="https://example.org/where-this-came-from"
          />
          <TextInput
            label="Created by"
            value={value.created_by}
            onChange={(s) => patch({ created_by: s })}
            disabled={disabled}
            placeholder="alice@lab.example"
          />
          <TextInput
            label="Description"
            value={value.description}
            onChange={(s) => patch({ description: s })}
            disabled={disabled}
            placeholder="Short description"
          />
        </div>
      </details>
    </div>
  );
}

function TextInput({
  label,
  value,
  onChange,
  disabled,
  placeholder,
}: {
  label: string;
  value: string;
  onChange: (s: string) => void;
  disabled?: boolean;
  placeholder?: string;
}) {
  return (
    <label className="block">
      <span className="mb-1.5 block text-sm font-medium text-foreground">
        {label}
      </span>
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        disabled={disabled}
        placeholder={placeholder}
        className="w-full rounded-md border bg-background px-3 py-2 text-sm text-foreground outline-none placeholder:text-muted-foreground/60 focus:ring-1 focus:ring-ring disabled:opacity-50"
      />
    </label>
  );
}

function FormatSelect({
  value,
  onChange,
  disabled,
}: {
  value: SerializationFormat;
  onChange: (s: SerializationFormat) => void;
  disabled?: boolean;
}) {
  return (
    <label className="block">
      <span className="mb-1.5 block text-sm font-medium text-foreground">
        Format
      </span>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value as SerializationFormat)}
        disabled={disabled}
        className="w-full rounded-md border bg-background px-3 py-2 text-sm text-foreground outline-none focus:ring-1 focus:ring-ring disabled:opacity-50"
      >
        {SERIALIZATION_FORMATS.map((f) => (
          <option key={f} value={f}>
            {serializationLabel(f)}
          </option>
        ))}
      </select>
    </label>
  );
}
