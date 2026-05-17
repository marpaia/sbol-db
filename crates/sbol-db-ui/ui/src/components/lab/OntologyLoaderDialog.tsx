/**
 * Modal for loading a new ontology. Two paths:
 *
 *  - Quick load: pick SO or SBO; the server fills in URL + name from
 *    its built-in defaults, so the user only needs one click.
 *  - Custom: prefix + URL (required) + name (optional). Useful for
 *    any OBO-format ontology that lives at a stable URL.
 *
 * Loads can take a while — downloading the OBO, parsing thousands of
 * terms, computing the transitive closure. We surface the in-flight
 * state and report term/closure/alias counts on success.
 */

import { useEffect, useMemo, useState } from "react";
import { Check, Loader2, TriangleAlert, X } from "lucide-react";

import {
  loadOntology,
  type OntologyLoadReport,
  type OntologyLoadRequest,
} from "@/lib/api";

export interface OntologyLoaderDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onLoaded: () => void;
  /** Prefixes already present in the corpus. Quick-pick buttons for
   *  these show as "Reload" with a small indicator so the user knows
   *  re-clicking will refetch rather than no-op. */
  loadedPrefixes?: string[];
}

type LoadState =
  | { kind: "idle" }
  | { kind: "loading"; label: string }
  | { kind: "loaded"; report: OntologyLoadReport }
  | { kind: "error"; message: string };

const QUICK_PICKS = [
  { prefix: "SO", label: "Sequence Ontology" },
  { prefix: "SBO", label: "Systems Biology Ontology" },
];

export function OntologyLoaderDialog({
  open,
  onOpenChange,
  onLoaded,
  loadedPrefixes = [],
}: OntologyLoaderDialogProps) {
  const [state, setState] = useState<LoadState>({ kind: "idle" });
  const [prefix, setPrefix] = useState("");
  const [url, setUrl] = useState("");
  const [name, setName] = useState("");

  const loadedSet = useMemo(
    () => new Set(loadedPrefixes.map((p) => p.toUpperCase())),
    [loadedPrefixes]
  );

  useEffect(() => {
    if (!open) {
      setState({ kind: "idle" });
      setPrefix("");
      setUrl("");
      setName("");
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

  const run = async (req: OntologyLoadRequest, label: string) => {
    setState({ kind: "loading", label });
    try {
      const report = await loadOntology(req);
      setState({ kind: "loaded", report });
      onLoaded();
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

  const submitCustom = () => {
    if (!prefix.trim()) return;
    run(
      {
        prefix: prefix.trim(),
        url: url.trim() || undefined,
        name: name.trim() || undefined,
      },
      prefix.trim().toUpperCase()
    );
  };

  return (
    <div
      role="dialog"
      aria-modal="true"
      onClick={() => onOpenChange(false)}
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/60 px-4 pt-24 backdrop-blur-sm"
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="w-full max-w-lg overflow-hidden rounded-lg border bg-popover text-popover-foreground shadow-2xl"
      >
        <header className="flex items-center gap-2 border-b px-5 py-3">
          <h2 className="text-sm font-medium">Load ontology</h2>
          <button
            type="button"
            onClick={() => onOpenChange(false)}
            aria-label="Close"
            className="ml-auto text-muted-foreground transition-colors hover:text-foreground"
          >
            <X size={16} />
          </button>
        </header>

        <div className="space-y-6 px-5 py-5">
          <section>
            <SectionLabel>Quick load</SectionLabel>
            <div className="grid grid-cols-2 gap-2">
              {QUICK_PICKS.map((pick) => {
                const alreadyLoaded = loadedSet.has(pick.prefix);
                const disabled = alreadyLoaded || state.kind === "loading";
                return (
                  <button
                    key={pick.prefix}
                    type="button"
                    disabled={disabled}
                    onClick={() => run({ prefix: pick.prefix }, pick.prefix)}
                    title={
                      alreadyLoaded
                        ? `${pick.prefix} is already loaded. Use Custom below to refetch.`
                        : `Fetch ${pick.label}`
                    }
                    className="rounded-md border bg-card px-3 py-2 text-left transition-colors hover:bg-accent disabled:cursor-not-allowed disabled:opacity-60 disabled:hover:bg-card"
                  >
                    <div className="flex items-center gap-1.5">
                      <span className="font-mono text-xs text-foreground">
                        {pick.prefix}
                      </span>
                      {alreadyLoaded && (
                        <span className="inline-flex items-center gap-0.5 text-xs text-success">
                          <Check size={12} />
                          loaded
                        </span>
                      )}
                    </div>
                    <div className="mt-1 text-sm text-foreground">
                      {pick.label}
                    </div>
                  </button>
                );
              })}
            </div>
          </section>

          <section>
            <SectionLabel>Custom</SectionLabel>
            <form
              className="space-y-3"
              onSubmit={(e) => {
                e.preventDefault();
                submitCustom();
              }}
            >
              <Field
                label="Prefix"
                placeholder="EDAM"
                value={prefix}
                onChange={setPrefix}
                disabled={state.kind === "loading"}
                required
              />
              <Field
                label="OBO URL"
                placeholder="http://purl.obolibrary.org/obo/edam.obo"
                value={url}
                onChange={setUrl}
                disabled={state.kind === "loading"}
                required
              />
              <Field
                label="Name (optional)"
                placeholder="EDAM Ontology"
                value={name}
                onChange={setName}
                disabled={state.kind === "loading"}
              />
              <div className="flex justify-end pt-1">
                <button
                  type="submit"
                  disabled={
                    state.kind === "loading" || !prefix.trim() || !url.trim()
                  }
                  className="rounded-md bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:bg-muted disabled:text-muted-foreground"
                >
                  Load
                </button>
              </div>
            </form>
          </section>

          <Status state={state} onDone={() => onOpenChange(false)} />
        </div>
      </div>
    </div>
  );
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <h3 className="mb-2 text-xs font-medium uppercase tracking-wider text-muted-foreground">
      {children}
    </h3>
  );
}

function Field({
  label,
  placeholder,
  value,
  onChange,
  disabled,
  required,
}: {
  label: string;
  placeholder: string;
  value: string;
  onChange: (s: string) => void;
  disabled?: boolean;
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
        placeholder={placeholder}
        disabled={disabled}
        className="w-full rounded-md border bg-background px-3 py-2 text-sm text-foreground outline-none placeholder:text-muted-foreground/60 focus:ring-1 focus:ring-ring disabled:opacity-50"
      />
    </label>
  );
}

function Status({ state, onDone }: { state: LoadState; onDone: () => void }) {
  if (state.kind === "idle") return null;
  if (state.kind === "loading") {
    return (
      <div className="flex items-center gap-2 rounded-md border bg-muted/40 px-3 py-2 text-sm text-foreground">
        <Loader2 size={14} className="animate-spin text-foreground" />
        <span>
          Downloading and parsing{" "}
          <span className="font-mono">{state.label}</span>… this can take a
          moment.
        </span>
      </div>
    );
  }
  if (state.kind === "error") {
    return (
      <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-foreground">
        <div className="flex items-center gap-2">
          <TriangleAlert size={14} className="text-destructive" />
          <span className="font-medium">Load failed</span>
        </div>
        <pre className="mt-2 whitespace-pre-wrap font-mono text-xs text-muted-foreground">
          {state.message}
        </pre>
      </div>
    );
  }
  const r = state.report;
  return (
    <div className="rounded-md border border-success/40 bg-success/5 px-3 py-3 text-sm">
      <div className="flex items-center justify-between gap-2">
        <span className="font-medium text-foreground">Loaded {r.prefix}</span>
        <button
          type="button"
          onClick={onDone}
          className="text-sm text-muted-foreground transition-colors hover:text-foreground"
        >
          Close
        </button>
      </div>
      <dl className="mt-3 grid grid-cols-3 gap-2">
        <Stat label="Terms" value={r.term_count} />
        <Stat label="Closure pairs" value={r.closure_count} />
        <Stat label="Aliases" value={r.alias_count} />
      </dl>
      {r.version && (
        <div className="mt-2 text-xs text-muted-foreground">
          version <span className="font-mono">{r.version}</span>
        </div>
      )}
    </div>
  );
}

function Stat({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-md bg-background px-2 py-1.5">
      <div className="text-xs text-muted-foreground">{label}</div>
      <div className="mt-0.5 text-base tabular-nums text-foreground">
        {value.toLocaleString()}
      </div>
    </div>
  );
}
