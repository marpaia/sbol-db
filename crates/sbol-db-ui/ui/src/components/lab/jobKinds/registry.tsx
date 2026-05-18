/**
 * Registry of structured forms for each job kind. Adding a new kind
 * means writing its form module (with `emptyValue`, `validate`, and
 * `toPayload`), then registering an entry here. The enqueue dialog
 * iterates this list to populate its kind selector.
 *
 * The form's internal value type stays in its own module to keep
 * generic plumbing simple — at the registry level we erase to
 * `unknown` so kinds with different payload shapes can coexist.
 */

import type { ReactNode } from "react";

import {
  ImportDocumentForm,
  importDocumentEmpty,
  importDocumentToPayload,
  importDocumentValidate,
  type ImportDocumentValue,
} from "./ImportDocumentForm";

export interface JobKindForm<V> {
  kind: string;
  label: string;
  description: string;
  emptyValue: () => V;
  validate: (value: V) => string | null;
  toPayload: (value: V) => unknown;
  Component: (props: {
    value: V;
    onChange: (next: V) => void;
    disabled?: boolean;
  }) => ReactNode;
}

export const JOB_KINDS: ReadonlyArray<JobKindForm<unknown>> = [
  {
    kind: "import_document",
    label: "Import document",
    description:
      "Async SBOL document ingest. Use the synchronous Import on the Documents page for small one-shot uploads.",
    emptyValue: importDocumentEmpty,
    validate: (v) => importDocumentValidate(v as ImportDocumentValue),
    toPayload: (v) => importDocumentToPayload(v as ImportDocumentValue),
    Component: ({ value, onChange, disabled }) => (
      <ImportDocumentForm
        value={value as ImportDocumentValue}
        onChange={(next) => onChange(next)}
        disabled={disabled}
      />
    ),
  } as JobKindForm<unknown>,
];

export function findKind(kind: string): JobKindForm<unknown> | undefined {
  return JOB_KINDS.find((k) => k.kind === kind);
}
