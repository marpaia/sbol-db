/**
 * SPARQL workbench. Editor on top, results on bottom, resizable. The
 * Run button (or ⌘↵) posts the buffer to the canonical /sparql
 * endpoint and reshapes the SPARQL JSON results into the generic
 * table model that ResultsTable consumes.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { Panel, PanelGroup, PanelResizeHandle } from "react-resizable-panels";
import { Play, Square } from "lucide-react";

import { EditorPane } from "@/components/lab/EditorPane";
import { ResultsPane, type ResultsState } from "@/components/lab/ResultsPane";
import { StatusBar } from "@/components/lab/StatusBar";
import { WorkbenchShell } from "@/components/lab/WorkbenchShell";
import {
  executeSparql,
  isSparqlAsk,
  isSparqlSelect,
  validateSparql,
  type SparqlBinding,
  type SparqlOutcome,
} from "@/lib/api";
import { downloadCsv, downloadJson } from "@/lib/export";
import { SPARQL_LANGUAGE_ID } from "@/lib/monaco/sparql-lang";
import { useLabStore } from "@/lib/store";

export default function SparqlRoute() {
  const buffer = useLabStore((s) => s.buffers.sparql);
  const setBuffer = useLabStore((s) => s.setBuffer);
  const pushHistory = useLabStore((s) => s.pushHistory);
  const setDialect = useLabStore((s) => s.setDialect);
  useEffect(() => setDialect("sparql"), [setDialect]);

  const [state, setState] = useState<ResultsState>({ kind: "idle" });
  const abortRef = useRef<AbortController | null>(null);

  const run = useCallback(async () => {
    abortRef.current?.abort();
    const ctrl = new AbortController();
    abortRef.current = ctrl;
    setState({ kind: "loading" });

    const startedAt = Date.now();
    try {
      const outcome = await executeSparql(buffer, ctrl.signal);
      const next = toResultsState(outcome);
      setState(next);
      pushHistory({
        dialect: "sparql",
        query: buffer,
        ranAt: startedAt,
        elapsedMs: outcome.elapsedMs,
        rowCount: rowCountOf(next) ?? 0,
        ok: true,
      });
    } catch (err) {
      if (ctrl.signal.aborted) {
        setState({ kind: "idle" });
        return;
      }
      const message = err instanceof Error ? err.message : "Unknown error";
      const detail =
        err && typeof err === "object" && "body" in err
          ? String((err as { body: unknown }).body)
          : undefined;
      setState({ kind: "error", message, detail });
      pushHistory({
        dialect: "sparql",
        query: buffer,
        ranAt: startedAt,
        elapsedMs: Date.now() - startedAt,
        rowCount: 0,
        ok: false,
        errorMessage: message,
      });
    } finally {
      if (abortRef.current === ctrl) abortRef.current = null;
    }
  }, [buffer, pushHistory]);

  const stop = useCallback(() => {
    abortRef.current?.abort();
  }, []);

  const running = state.kind === "loading";

  const insertIntoEditor = (text: string) => {
    const existing = useLabStore.getState().buffers.sparql;
    const next =
      existing.length === 0 || existing.endsWith("\n")
        ? `${existing}${text}`
        : `${existing} ${text}`;
    setBuffer("sparql", next);
  };

  return (
    <WorkbenchShell
      dialect="sparql"
      currentBuffer={buffer}
      onInsertIntoEditor={insertIntoEditor}
      onLoadQuery={(q) => setBuffer("sparql", q)}
    >
      <div className="h-full w-full flex flex-col min-h-0">
        <Toolbar onRun={run} onStop={stop} running={running} />
        <PanelGroup direction="vertical" className="flex-1 min-h-0 h-full">
          <Panel defaultSize={55} minSize={20}>
            <EditorPane
              language={SPARQL_LANGUAGE_ID}
              value={buffer}
              onChange={(t) => setBuffer("sparql", t)}
              onRun={run}
              validate={validateSparql}
            />
          </Panel>
          <PanelResizeHandle className="h-px bg-border transition-colors hover:bg-ring/40" />
          <Panel defaultSize={45} minSize={15}>
            <ResultsPane state={state} />
          </Panel>
        </PanelGroup>
        <StatusBar
          dialect="SPARQL"
          status={statusOf(state)}
          rowCount={rowCountOf(state)}
          elapsedMs={elapsedOf(state)}
          truncated={state.kind === "table" ? state.truncated : false}
          onExportCsv={
            state.kind === "table"
              ? () =>
                  downloadCsv(
                    state.columns,
                    state.rows,
                    `sbol-lab-${Date.now()}.csv`
                  )
              : undefined
          }
          onExportJson={
            state.kind === "table"
              ? () =>
                  downloadJson(
                    state.columns,
                    state.rows,
                    `sbol-lab-${Date.now()}.json`
                  )
              : undefined
          }
        />
      </div>
    </WorkbenchShell>
  );
}

function Toolbar({
  onRun,
  onStop,
  running,
}: {
  onRun: () => void;
  onStop: () => void;
  running: boolean;
}) {
  return (
    <div className="flex items-center gap-2 border-b bg-background px-3 py-1.5">
      {running ? (
        <button
          type="button"
          onClick={onStop}
          className="inline-flex items-center gap-1.5 rounded-md bg-destructive px-3 py-1 text-xs font-medium text-destructive-foreground transition-colors hover:bg-destructive/90"
        >
          <Square size={12} />
          <span>Stop</span>
        </button>
      ) : (
        <button
          type="button"
          onClick={onRun}
          className="inline-flex items-center gap-1.5 rounded-md bg-primary px-3 py-1 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90"
        >
          <Play size={12} />
          <span>Run</span>
          <kbd className="ml-1 text-[10px] text-primary-foreground/70">⌘↵</kbd>
        </button>
      )}
    </div>
  );
}

function toResultsState(outcome: SparqlOutcome): ResultsState {
  if (isSparqlAsk(outcome.body)) {
    return {
      kind: "ask",
      value: outcome.body.boolean,
      elapsedMs: outcome.elapsedMs,
    };
  }
  if (isSparqlSelect(outcome.body)) {
    const vars = outcome.body.head.vars;
    const columns = vars.map((v) => ({ name: v, typeHint: "binding" }));
    const rows = outcome.body.results.bindings.map((b) =>
      vars.map((v) => bindingValue(b[v]))
    );
    return {
      kind: "table",
      columns,
      rows,
      truncated: outcome.truncated,
      elapsedMs: outcome.elapsedMs,
    };
  }
  // CONSTRUCT / DESCRIBE → raw Turtle (or whatever the engine picked).
  return {
    kind: "raw",
    mime: outcome.contentType,
    body:
      typeof outcome.body === "string"
        ? outcome.body
        : JSON.stringify(outcome.body, null, 2),
    elapsedMs: outcome.elapsedMs,
  };
}

function bindingValue(b: SparqlBinding | undefined): string | null {
  if (!b) return null;
  return b.value;
}

function statusOf(s: ResultsState): "idle" | "running" | "ok" | "error" {
  switch (s.kind) {
    case "idle":
      return "idle";
    case "loading":
      return "running";
    case "error":
      return "error";
    default:
      return "ok";
  }
}

function rowCountOf(s: ResultsState): number | undefined {
  if (s.kind === "table") return s.rows.length;
  if (s.kind === "ask") return 1;
  return undefined;
}

function elapsedOf(s: ResultsState): number | undefined {
  if (s.kind === "table" || s.kind === "ask" || s.kind === "raw")
    return s.elapsedMs;
  return undefined;
}
