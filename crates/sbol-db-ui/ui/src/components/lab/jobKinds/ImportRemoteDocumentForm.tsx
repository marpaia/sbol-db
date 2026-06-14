import {
  IMPORT_DOCUMENT_FORMATS,
  importFormatLabel,
  type ImportDocumentFormat,
} from "@/lib/api";

import type { ImportRemoteDocumentValue } from "./importRemoteDocument";

export interface ImportRemoteDocumentFormProps {
  value: ImportRemoteDocumentValue;
  onChange: (v: ImportRemoteDocumentValue) => void;
  disabled?: boolean;
}

export function ImportRemoteDocumentForm({
  value,
  onChange,
  disabled,
}: ImportRemoteDocumentFormProps) {
  const patch = (delta: Partial<ImportRemoteDocumentValue>) =>
    onChange({ ...value, ...delta });

  return (
    <div className="space-y-3">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-[1fr,160px]">
        <TextInput
          label="URL"
          value={value.url}
          onChange={(s) => patch({ url: s })}
          disabled={disabled}
          placeholder="https://synbiohub.org/public/igem/BBa_B0034/1/sbol"
          required
        />
        <FormatSelect
          value={value.format}
          onChange={(s) => patch({ format: s })}
          disabled={disabled}
        />
      </div>

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
            label="Namespace"
            value={value.namespace}
            onChange={(s) => patch({ namespace: s })}
            disabled={disabled}
            placeholder="https://example.org/lab"
          />
          <TextInput
            label="Created by"
            value={value.created_by}
            onChange={(s) => patch({ created_by: s })}
            disabled={disabled}
            placeholder="alice@lab.example"
          />
          <TextInput
            label="Name"
            value={value.name}
            onChange={(s) => patch({ name: s })}
            disabled={disabled}
            placeholder="Display name"
          />
          <div className="sm:col-span-2">
            <TextInput
              label="Description"
              value={value.description}
              onChange={(s) => patch({ description: s })}
              disabled={disabled}
              placeholder="Short description"
            />
          </div>
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
  required,
}: {
  label: string;
  value: string;
  onChange: (s: string) => void;
  disabled?: boolean;
  placeholder?: string;
  required?: boolean;
}) {
  return (
    <label className="block">
      <span className="mb-1.5 block text-sm font-medium text-foreground">
        {label}
        {required && <span className="ml-1 text-destructive">*</span>}
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
  value: ImportDocumentFormat;
  onChange: (s: ImportDocumentFormat) => void;
  disabled?: boolean;
}) {
  return (
    <label className="block">
      <span className="mb-1.5 block text-sm font-medium text-foreground">
        Format
      </span>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value as ImportDocumentFormat)}
        disabled={disabled}
        className="w-full rounded-md border bg-background px-3 py-2 text-sm text-foreground outline-none focus:ring-1 focus:ring-ring disabled:opacity-50"
      >
        {IMPORT_DOCUMENT_FORMATS.map((f) => (
          <option key={f} value={f}>
            {importFormatLabel(f)}
          </option>
        ))}
      </select>
    </label>
  );
}
