/**
 * Modal for importing an SBOL document. Two paths:
 *
 *  - Upload: drop a Turtle / JSON-LD / RDF-XML / N-Triples file. The
 *    format is inferred from the extension and the file body is read
 *    into a string so the POST mirrors the paste path exactly.
 *  - Paste: a textarea + a format dropdown. Same submit code path.
 *
 * Optional metadata fields (name, description, source URI, document
 * IRI, created by) are folded into query parameters. On success we
 * surface the ImportReport (object count, quad count, validation
 * status) and offer a button to jump to the new detail page.
 */

import { useEffect, useState } from "react";
import { Check, Loader2, TriangleAlert, Upload, X } from "lucide-react";

import {
  importDocument,
  SERIALIZATION_FORMATS,
  serializationLabel,
  type ImportReport,
  type SerializationFormat,
} from "@/lib/api";
import { describeError } from "@/lib/utils";

export interface DocumentImportDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Called once the import succeeds. Receives the new document id. */
  onImported: (report: ImportReport) => void;
}

type Tab = "paste" | "upload";

type Phase =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "loaded"; report: ImportReport }
  | { kind: "error"; message: string };

const EXTENSION_FORMAT: Record<string, SerializationFormat> = {
  ttl: "turtle",
  turtle: "turtle",
  jsonld: "jsonld",
  json: "jsonld",
  rdf: "rdfxml",
  xml: "rdfxml",
  nt: "ntriples",
  ntriples: "ntriples",
};

function formatFromFilename(name: string): SerializationFormat | null {
  const ext = name.split(".").pop()?.toLowerCase();
  if (!ext) return null;
  return EXTENSION_FORMAT[ext] ?? null;
}

export function DocumentImportDialog({
  open,
  onOpenChange,
  onImported,
}: DocumentImportDialogProps) {
  const [tab, setTab] = useState<Tab>("paste");
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  const [format, setFormat] = useState<SerializationFormat>("turtle");
  const [body, setBody] = useState("");
  const [fileName, setFileName] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [sourceUri, setSourceUri] = useState("");
  const [documentIri, setDocumentIri] = useState("");
  const [createdBy, setCreatedBy] = useState("");

  useEffect(() => {
    if (!open) {
      setTab("paste");
      setPhase({ kind: "idle" });
      setFormat("turtle");
      setBody("");
      setFileName(null);
      setName("");
      setDescription("");
      setSourceUri("");
      setDocumentIri("");
      setCreatedBy("");
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

  const onFile = async (file: File) => {
    const inferred = formatFromFilename(file.name);
    if (inferred) setFormat(inferred);
    setFileName(file.name);
    const text = await file.text();
    setBody(text);
  };

  const submit = async () => {
    if (!body.trim()) return;
    setPhase({ kind: "loading" });
    try {
      const report = await importDocument({
        format,
        body,
        name: name.trim() || undefined,
        description: description.trim() || undefined,
        source_uri: sourceUri.trim() || undefined,
        document_iri: documentIri.trim() || undefined,
        created_by: createdBy.trim() || undefined,
      });
      setPhase({ kind: "loaded", report });
      onImported(report);
    } catch (err) {
      setPhase({ kind: "error", message: describeError(err) });
    }
  };

  const canSubmit = body.trim().length > 0 && phase.kind !== "loading";

  return (
    <div
      role="dialog"
      aria-modal="true"
      onClick={() => onOpenChange(false)}
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/60 px-4 pt-16 backdrop-blur-sm"
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="w-full max-w-2xl overflow-hidden rounded-lg border bg-popover text-popover-foreground shadow-2xl"
      >
        <header className="flex items-center gap-2 border-b px-5 py-3">
          <h2 className="text-sm font-medium">Import SBOL document</h2>
          <button
            type="button"
            onClick={() => onOpenChange(false)}
            aria-label="Close"
            className="ml-auto text-muted-foreground transition-colors hover:text-foreground"
          >
            <X size={16} />
          </button>
        </header>

        <div className="space-y-5 px-5 py-5">
          <div className="flex items-center gap-1 border-b">
            <TabButton active={tab === "paste"} onClick={() => setTab("paste")}>
              Paste
            </TabButton>
            <TabButton
              active={tab === "upload"}
              onClick={() => setTab("upload")}
            >
              Upload file
            </TabButton>
            <div className="ml-auto flex items-center gap-2 pb-2">
              <label className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
                Format
              </label>
              <select
                value={format}
                onChange={(e) =>
                  setFormat(e.target.value as SerializationFormat)
                }
                disabled={phase.kind === "loading"}
                className="rounded-md border bg-background px-2 py-1 text-xs text-foreground outline-none focus:ring-1 focus:ring-ring disabled:opacity-50"
              >
                {SERIALIZATION_FORMATS.map((f) => (
                  <option key={f} value={f}>
                    {serializationLabel(f)}
                  </option>
                ))}
              </select>
            </div>
          </div>

          {tab === "paste" ? (
            <textarea
              value={body}
              onChange={(e) => setBody(e.target.value)}
              placeholder="@prefix sbol: <http://sbols.org/v3#> ."
              disabled={phase.kind === "loading"}
              spellCheck={false}
              rows={10}
              className="block w-full resize-y rounded-md border bg-background px-3 py-2 font-mono text-xs text-foreground outline-none placeholder:text-muted-foreground/60 focus:ring-1 focus:ring-ring disabled:opacity-50"
            />
          ) : (
            <FileDrop
              fileName={fileName}
              onFile={onFile}
              onClear={() => {
                setFileName(null);
                setBody("");
              }}
              disabled={phase.kind === "loading"}
              bodyPreview={body}
            />
          )}

          <details className="rounded-md border bg-card">
            <summary className="cursor-pointer select-none px-3 py-2 text-xs font-medium text-muted-foreground hover:text-foreground">
              Optional metadata
            </summary>
            <div className="grid gap-3 px-3 py-3 sm:grid-cols-2">
              <Field
                label="Name"
                value={name}
                onChange={setName}
                placeholder="Display name"
                disabled={phase.kind === "loading"}
              />
              <Field
                label="Created by"
                value={createdBy}
                onChange={setCreatedBy}
                placeholder="mike@arpaia.co"
                disabled={phase.kind === "loading"}
              />
              <Field
                label="Source URI"
                value={sourceUri}
                onChange={setSourceUri}
                placeholder="https://…"
                disabled={phase.kind === "loading"}
              />
              <Field
                label="Document IRI"
                value={documentIri}
                onChange={setDocumentIri}
                placeholder="http://…"
                disabled={phase.kind === "loading"}
              />
              <div className="sm:col-span-2">
                <Field
                  label="Description"
                  value={description}
                  onChange={setDescription}
                  placeholder="What's in this document?"
                  disabled={phase.kind === "loading"}
                />
              </div>
            </div>
          </details>

          <div className="flex items-center justify-end gap-2">
            <button
              type="button"
              onClick={() => onOpenChange(false)}
              className="rounded-md px-3 py-1.5 text-sm text-muted-foreground transition-colors hover:text-foreground"
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={submit}
              disabled={!canSubmit}
              className="rounded-md bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:bg-muted disabled:text-muted-foreground"
            >
              Import
            </button>
          </div>

          <Status phase={phase} onDone={() => onOpenChange(false)} />
        </div>
      </div>
    </div>
  );
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`-mb-px border-b-2 px-3 py-1.5 text-xs font-medium transition-colors ${
        active
          ? "border-primary text-foreground"
          : "border-transparent text-muted-foreground hover:text-foreground"
      }`}
    >
      {children}
    </button>
  );
}

function FileDrop({
  fileName,
  onFile,
  onClear,
  disabled,
  bodyPreview,
}: {
  fileName: string | null;
  onFile: (file: File) => Promise<void> | void;
  onClear: () => void;
  disabled?: boolean;
  bodyPreview: string;
}) {
  const [drag, setDrag] = useState(false);
  return (
    <div className="space-y-2">
      <label
        onDragEnter={(e) => {
          e.preventDefault();
          if (!disabled) setDrag(true);
        }}
        onDragOver={(e) => {
          e.preventDefault();
          if (!disabled) setDrag(true);
        }}
        onDragLeave={() => setDrag(false)}
        onDrop={(e) => {
          e.preventDefault();
          setDrag(false);
          const file = e.dataTransfer.files?.[0];
          if (file) void onFile(file);
        }}
        className={`flex h-32 cursor-pointer flex-col items-center justify-center gap-1.5 rounded-md border-2 border-dashed text-xs transition-colors ${
          disabled
            ? "border-muted text-muted-foreground/50"
            : drag
              ? "border-primary bg-primary/5 text-foreground"
              : "border-border text-muted-foreground hover:border-foreground/40 hover:text-foreground"
        }`}
      >
        <Upload size={16} />
        <div>Drop a file here, or click to browse</div>
        <div className="text-[10px] text-muted-foreground/70">
          .ttl · .jsonld · .rdf · .nt
        </div>
        <input
          type="file"
          accept=".ttl,.turtle,.jsonld,.json,.rdf,.xml,.nt"
          className="hidden"
          disabled={disabled}
          onChange={(e) => {
            const file = e.target.files?.[0];
            if (file) void onFile(file);
            e.target.value = "";
          }}
        />
      </label>
      {fileName && (
        <div className="flex items-center justify-between gap-2 rounded-md border bg-muted/40 px-3 py-2 text-xs">
          <div className="min-w-0 truncate">
            <span className="font-medium text-foreground">{fileName}</span>
            <span className="ml-2 text-muted-foreground">
              {bodyPreview.length.toLocaleString()} chars
            </span>
          </div>
          <button
            type="button"
            onClick={onClear}
            disabled={disabled}
            className="shrink-0 text-muted-foreground transition-colors hover:text-foreground disabled:opacity-50"
            aria-label="Clear"
          >
            <X size={12} />
          </button>
        </div>
      )}
    </div>
  );
}

function Field({
  label,
  value,
  onChange,
  placeholder,
  disabled,
}: {
  label: string;
  value: string;
  onChange: (s: string) => void;
  placeholder?: string;
  disabled?: boolean;
}) {
  return (
    <label className="block">
      <span className="mb-1 block text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
        {label}
      </span>
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        disabled={disabled}
        className="w-full rounded-md border bg-background px-3 py-1.5 text-xs text-foreground outline-none placeholder:text-muted-foreground/60 focus:ring-1 focus:ring-ring disabled:opacity-50"
      />
    </label>
  );
}

function Status({ phase, onDone }: { phase: Phase; onDone: () => void }) {
  if (phase.kind === "idle") return null;
  if (phase.kind === "loading") {
    return (
      <div className="flex items-center gap-2 rounded-md border bg-muted/40 px-3 py-2 text-sm text-foreground">
        <Loader2 size={14} className="animate-spin" />
        <span>Parsing and persisting…</span>
      </div>
    );
  }
  if (phase.kind === "error") {
    return (
      <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm">
        <div className="flex items-center gap-2">
          <TriangleAlert size={14} className="text-destructive" />
          <span className="font-medium text-foreground">Import failed</span>
        </div>
        <pre className="mt-2 whitespace-pre-wrap font-mono text-xs text-muted-foreground">
          {phase.message}
        </pre>
      </div>
    );
  }
  const r = phase.report;
  const failed = r.validation_status === "failed";
  return (
    <div
      className={`rounded-md border px-3 py-3 text-sm ${
        failed
          ? "border-destructive/40 bg-destructive/5"
          : "border-success/40 bg-success/5"
      }`}
    >
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <Check
            size={14}
            className={failed ? "text-destructive" : "text-success"}
          />
          <span className="font-medium text-foreground">
            {failed ? "Imported with validation issues" : "Imported"}
          </span>
        </div>
        <button
          type="button"
          onClick={onDone}
          className="text-sm text-muted-foreground transition-colors hover:text-foreground"
        >
          Close
        </button>
      </div>
      <dl className="mt-3 grid grid-cols-3 gap-2">
        <Stat label="Objects" value={r.object_count} />
        <Stat label="Quads" value={r.quad_count} />
        <Stat
          label="Validation"
          value={r.validation_issue_count}
          suffix={failed ? "issues" : "passed"}
        />
      </dl>
    </div>
  );
}

function Stat({
  label,
  value,
  suffix,
}: {
  label: string;
  value: number;
  suffix?: string;
}) {
  return (
    <div className="rounded-md bg-background px-2 py-1.5">
      <div className="text-xs text-muted-foreground">{label}</div>
      <div className="mt-0.5 text-base tabular-nums text-foreground">
        {value.toLocaleString()}
        {suffix && (
          <span className="ml-1 text-[10px] font-normal uppercase tracking-wider text-muted-foreground">
            {suffix}
          </span>
        )}
      </div>
    </div>
  );
}
