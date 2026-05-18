/**
 * Modal for enqueueing a background job. Each registered kind brings
 * its own structured form (see `jobKinds/registry.tsx`); the kind
 * selector at the top switches between them. The "Advanced" section
 * exposes queue, priority, max_attempts, idempotency_key, and
 * correlation_id, which are kind-agnostic.
 *
 * On submit, the kind's `validate` runs, then `toPayload` serializes
 * the form value to the JSON shape the server expects, then we POST
 * `/jobs`.
 */

import { useEffect, useState } from "react";
import { Loader2, TriangleAlert, X } from "lucide-react";

import {
  enqueueJob,
  type EnqueueJobRequest,
  type EnqueueJobResult,
} from "@/lib/api";

import { JOB_KINDS, findKind } from "./jobKinds/registry";

export interface EnqueueJobDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onEnqueued: (result: EnqueueJobResult) => void;
}

type SubmitState =
  | { kind: "idle" }
  | { kind: "submitting" }
  | { kind: "error"; message: string }
  | { kind: "done"; result: EnqueueJobResult };

export function EnqueueJobDialog({
  open,
  onOpenChange,
  onEnqueued,
}: EnqueueJobDialogProps) {
  const [selectedKind, setSelectedKind] = useState(JOB_KINDS[0].kind);
  const [formValue, setFormValue] = useState<unknown>(() =>
    JOB_KINDS[0].emptyValue()
  );
  const [queue, setQueue] = useState("");
  const [priority, setPriority] = useState("");
  const [maxAttempts, setMaxAttempts] = useState("");
  const [idempotencyKey, setIdempotencyKey] = useState("");
  const [correlationId, setCorrelationId] = useState("");
  const [state, setState] = useState<SubmitState>({ kind: "idle" });

  const kindEntry = findKind(selectedKind);

  useEffect(() => {
    if (!open) {
      setState({ kind: "idle" });
      setSelectedKind(JOB_KINDS[0].kind);
      setFormValue(JOB_KINDS[0].emptyValue());
      setQueue("");
      setPriority("");
      setMaxAttempts("");
      setIdempotencyKey("");
      setCorrelationId("");
    }
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onOpenChange(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onOpenChange]);

  if (!open) return null;
  if (!kindEntry) return null;

  const switchKind = (next: string) => {
    const entry = findKind(next);
    if (!entry) return;
    setSelectedKind(next);
    setFormValue(entry.emptyValue());
    setState({ kind: "idle" });
  };

  const submit = async () => {
    const validationError = kindEntry.validate(formValue);
    if (validationError) {
      setState({ kind: "error", message: validationError });
      return;
    }

    const req: EnqueueJobRequest = {
      kind: kindEntry.kind,
      payload: kindEntry.toPayload(formValue),
    };
    if (queue.trim()) req.queue = queue.trim();
    if (priority.trim()) {
      const n = Number(priority);
      if (!Number.isFinite(n) || !Number.isInteger(n)) {
        setState({ kind: "error", message: "Priority must be an integer." });
        return;
      }
      req.priority = n;
    }
    if (maxAttempts.trim()) {
      const n = Number(maxAttempts);
      if (!Number.isFinite(n) || !Number.isInteger(n) || n < 1) {
        setState({
          kind: "error",
          message: "Max attempts must be a positive integer.",
        });
        return;
      }
      req.max_attempts = n;
    }
    if (idempotencyKey.trim()) req.idempotency_key = idempotencyKey.trim();
    if (correlationId.trim()) req.correlation_id = correlationId.trim();

    setState({ kind: "submitting" });
    try {
      const result = await enqueueJob(req);
      setState({ kind: "done", result });
      onEnqueued(result);
    } catch (err) {
      const message =
        err && typeof err === "object" && "body" in err
          ? String((err as { body: unknown }).body)
          : err instanceof Error
            ? err.message
            : "Unknown error";
      setState({ kind: "error", message });
    }
  };

  const submitting = state.kind === "submitting";

  return (
    <div
      role="dialog"
      aria-modal="true"
      onClick={() => onOpenChange(false)}
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/60 px-4 pt-16 backdrop-blur-sm"
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="max-h-[85vh] w-full max-w-2xl overflow-hidden rounded-lg border bg-popover text-popover-foreground shadow-2xl flex flex-col"
      >
        <header className="flex items-center gap-2 border-b px-5 py-3">
          <h2 className="text-sm font-medium">Enqueue job</h2>
          <button
            type="button"
            onClick={() => onOpenChange(false)}
            aria-label="Close"
            className="ml-auto text-muted-foreground transition-colors hover:text-foreground"
          >
            <X size={16} />
          </button>
        </header>

        <form
          className="flex-1 space-y-4 overflow-y-auto px-5 py-5"
          onSubmit={(e) => {
            e.preventDefault();
            submit();
          }}
        >
          <KindSelector
            value={selectedKind}
            onChange={switchKind}
            disabled={submitting}
          />

          <kindEntry.Component
            value={formValue}
            onChange={(next) => setFormValue(next)}
            disabled={submitting}
          />

          <details className="rounded-md border bg-card">
            <summary className="cursor-pointer select-none px-3 py-2 text-xs font-medium uppercase tracking-wider text-muted-foreground">
              Queue settings
            </summary>
            <div className="grid gap-3 border-t px-3 py-3 sm:grid-cols-2">
              <TextField
                label="Queue"
                value={queue}
                onChange={setQueue}
                disabled={submitting}
                placeholder="default"
              />
              <TextField
                label="Priority"
                value={priority}
                onChange={setPriority}
                disabled={submitting}
                placeholder="0"
                inputMode="numeric"
              />
              <TextField
                label="Max attempts"
                value={maxAttempts}
                onChange={setMaxAttempts}
                disabled={submitting}
                placeholder="5"
                inputMode="numeric"
              />
              <TextField
                label="Idempotency key"
                value={idempotencyKey}
                onChange={setIdempotencyKey}
                disabled={submitting}
                placeholder="(dedupes against an existing job)"
              />
              <div className="sm:col-span-2">
                <TextField
                  label="Correlation ID"
                  value={correlationId}
                  onChange={setCorrelationId}
                  disabled={submitting}
                  placeholder="UUID"
                />
              </div>
            </div>
          </details>

          <div className="flex items-center justify-end gap-2 pt-1">
            <button
              type="button"
              onClick={() => onOpenChange(false)}
              disabled={submitting}
              className="rounded-md border bg-background px-3 py-1.5 text-sm font-medium text-foreground transition-colors hover:bg-accent disabled:opacity-50"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={submitting}
              className="inline-flex items-center gap-1.5 rounded-md bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:bg-muted disabled:text-muted-foreground"
            >
              {submitting && <Loader2 size={12} className="animate-spin" />}
              Enqueue
            </button>
          </div>

          <Status state={state} />
        </form>
      </div>
    </div>
  );
}

function KindSelector({
  value,
  onChange,
  disabled,
}: {
  value: string;
  onChange: (kind: string) => void;
  disabled?: boolean;
}) {
  const current = findKind(value);
  return (
    <div className="space-y-1.5">
      <label className="block text-sm font-medium text-foreground">Kind</label>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        disabled={disabled || JOB_KINDS.length <= 1}
        className="w-full rounded-md border bg-background px-3 py-2 text-sm text-foreground outline-none focus:ring-1 focus:ring-ring disabled:opacity-70"
      >
        {JOB_KINDS.map((k) => (
          <option key={k.kind} value={k.kind}>
            {k.label}
          </option>
        ))}
      </select>
      {current?.description && (
        <p className="text-[11px] text-muted-foreground">
          {current.description}
        </p>
      )}
    </div>
  );
}

function TextField({
  label,
  value,
  onChange,
  disabled,
  placeholder,
  inputMode,
}: {
  label: string;
  value: string;
  onChange: (s: string) => void;
  disabled?: boolean;
  placeholder?: string;
  inputMode?: "text" | "numeric";
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
        inputMode={inputMode}
        className="w-full rounded-md border bg-background px-3 py-2 text-sm text-foreground outline-none placeholder:text-muted-foreground/60 focus:ring-1 focus:ring-ring disabled:opacity-50"
      />
    </label>
  );
}

function Status({ state }: { state: SubmitState }) {
  if (state.kind === "idle" || state.kind === "submitting") return null;
  if (state.kind === "error") {
    return (
      <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm">
        <div className="flex items-center gap-2">
          <TriangleAlert size={14} className="text-destructive" />
          <span className="font-medium text-foreground">Enqueue failed</span>
        </div>
        <pre className="mt-2 whitespace-pre-wrap font-mono text-xs text-muted-foreground">
          {state.message}
        </pre>
      </div>
    );
  }
  const { job, deduplicated } = state.result;
  return (
    <div className="rounded-md border border-success/40 bg-success/5 px-3 py-2 text-sm">
      <div className="font-medium text-foreground">
        {deduplicated
          ? "Returned existing job (idempotency match)"
          : "Enqueued"}
      </div>
      <div className="mt-1 font-mono text-[11px] text-muted-foreground">
        {job.id}
      </div>
    </div>
  );
}
