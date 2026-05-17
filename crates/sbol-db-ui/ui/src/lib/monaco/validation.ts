/**
 * Shared marker-provider plumbing for the SQL and SPARQL editors.
 *
 * `attachValidator` listens for model changes, debounces, calls the
 * server-side parser, and applies the resulting errors as Monaco
 * markers (red squigglies + hover tooltips). The same machinery
 * handles both dialects — only the validator function differs.
 *
 * The provider also cancels in-flight requests when the buffer
 * changes again before the previous validation lands, so editor
 * keystrokes never race with stale responses.
 */

import type * as MonacoNS from "monaco-editor";

import type { ValidateError, ValidateResponse } from "@/lib/api";

const MARKER_OWNER = "sbol-lab";
const DEBOUNCE_MS = 250;

export type Validator = (
  query: string,
  signal: AbortSignal
) => Promise<ValidateResponse>;

export interface AttachedValidator {
  /** Force a re-validate right now (skips the debounce). Useful when
   *  the buffer changes externally — e.g., loading a saved query. */
  refresh: () => void;
  dispose: () => void;
}

export function attachValidator(
  monaco: typeof MonacoNS,
  editor: MonacoNS.editor.IStandaloneCodeEditor,
  validate: Validator
): AttachedValidator {
  const model = editor.getModel();
  if (!model) {
    return { refresh: () => {}, dispose: () => {} };
  }

  let pending: AbortController | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let lastValidatedVersion = -1;
  let disposed = false;

  const run = async () => {
    if (disposed) return;
    if (timer) {
      clearTimeout(timer);
      timer = null;
    }
    const value = model.getValue();
    const versionId = model.getVersionId();
    if (versionId === lastValidatedVersion) return;
    pending?.abort();
    const ctrl = new AbortController();
    pending = ctrl;
    try {
      const result = await validate(value, ctrl.signal);
      if (ctrl.signal.aborted || disposed) return;
      // Drop the result if the model has moved on; we'll be
      // re-validated by the in-flight follow-up.
      if (model.getVersionId() !== versionId) return;
      lastValidatedVersion = versionId;
      applyMarkers(monaco, model, result.errors);
    } catch (err) {
      if (ctrl.signal.aborted || disposed) return;
      // Network glitches shouldn't decorate the buffer; just clear
      // existing markers so the editor doesn't lie about stale errors.
      monaco.editor.setModelMarkers(model, MARKER_OWNER, []);
      console.warn("[lab] validation failed:", err);
    } finally {
      if (pending === ctrl) pending = null;
    }
  };

  const schedule = () => {
    if (disposed) return;
    if (timer) clearTimeout(timer);
    timer = setTimeout(run, DEBOUNCE_MS);
  };

  const sub = model.onDidChangeContent(() => schedule());
  // Validate the initial buffer once so a freshly opened tab gets
  // markers without waiting for the first keystroke.
  schedule();

  return {
    refresh: schedule,
    dispose: () => {
      disposed = true;
      sub.dispose();
      if (timer) clearTimeout(timer);
      pending?.abort();
      monaco.editor.setModelMarkers(model, MARKER_OWNER, []);
    },
  };
}

function applyMarkers(
  monaco: typeof MonacoNS,
  model: MonacoNS.editor.ITextModel,
  errors: ValidateError[]
): void {
  const markers: MonacoNS.editor.IMarkerData[] = errors.map((e) => {
    const startLineNumber = Math.max(1, e.line);
    const startColumn = Math.max(1, e.column);
    const endLineNumber = e.end_line ?? startLineNumber;
    const endColumn =
      e.end_column ?? endOfTokenColumn(model, startLineNumber, startColumn);
    return {
      severity: monaco.MarkerSeverity.Error,
      message: e.message,
      startLineNumber,
      startColumn,
      endLineNumber,
      endColumn,
      source: "sbol-lab",
    };
  });
  monaco.editor.setModelMarkers(model, MARKER_OWNER, markers);
}

/** Walk forward from `column` on `line` until a word boundary so the
 *  squiggly underlines the offending token rather than a single
 *  character. Caps at 32 cols to avoid weird ranges on whitespace. */
function endOfTokenColumn(
  model: MonacoNS.editor.ITextModel,
  line: number,
  column: number
): number {
  const text = model.getLineContent(line);
  let i = column - 1;
  const max = Math.min(text.length, i + 32);
  // If we're sitting on whitespace, just bump by one so the marker
  // is visible.
  if (i >= text.length || /\s/.test(text[i])) return column + 1;
  while (i < max && /[A-Za-z0-9_$.]/.test(text[i])) i += 1;
  return Math.max(column + 1, i + 1);
}
