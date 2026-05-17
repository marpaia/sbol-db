/**
 * SQL workbench. Editor on top, results on bottom; ⌘↵ runs the buffer
 * against /lab/api/sql/execute and renders the typed result set in
 * the shared ResultsTable.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { Panel, PanelGroup, PanelResizeHandle } from "react-resizable-panels";
import { Play, Square } from "lucide-react";

import { EditorPane } from "@/components/lab/EditorPane";
import { ResultsPane, type ResultsState } from "@/components/lab/ResultsPane";
import { StatusBar } from "@/components/lab/StatusBar";
import { WorkbenchShell } from "@/components/lab/WorkbenchShell";
import { executeSql, validateSql, type SqlExecuteResponse } from "@/lib/api";
import { downloadCsv, downloadJson } from "@/lib/export";
import { SQL_LANGUAGE_ID } from "@/lib/monaco/sql-lang";
import { useLabStore } from "@/lib/store";

export default function SqlRoute() {
  const buffer = useLabStore((s) => s.buffers.sql);
  const setBuffer = useLabStore((s) => s.setBuffer);
  const pushHistory = useLabStore((s) => s.pushHistory);
  const setDialect = useLabStore((s) => s.setDialect);
  useEffect(() => setDialect("sql"), [setDialect]);

  const [state, setState] = useState<ResultsState>({ kind: "idle" });
  const abortRef = useRef<AbortController | null>(null);

  const run = useCallback(async () => {
    abortRef.current?.abort();
    const ctrl = new AbortController();
    abortRef.current = ctrl;
    setState({ kind: "loading" });

    const startedAt = Date.now();
    try {
      const resp = await executeSql({ query: buffer }, ctrl.signal);
      setState(toResultsState(resp));
      pushHistory({
        dialect: "sql",
        query: buffer,
        ranAt: startedAt,
        elapsedMs: resp.elapsed_ms,
        rowCount: resp.rows.length,
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
        dialect: "sql",
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
    const existing = useLabStore.getState().buffers.sql;
    const next =
      existing.length === 0 || existing.endsWith("\n")
        ? `${existing}${text}`
        : `${existing} ${text}`;
    setBuffer("sql", next);
  };

  return (
    <WorkbenchShell
      dialect="sql"
      currentBuffer={buffer}
      onInsertIntoEditor={insertIntoEditor}
      onLoadQuery={(q) => setBuffer("sql", q)}
    >
      <div className="h-full w-full flex flex-col min-h-0">
        <Toolbar onRun={run} onStop={stop} running={running} />
        <PanelGroup direction="vertical" className="flex-1 min-h-0 h-full">
          <Panel defaultSize={55} minSize={20}>
            <EditorPane
              language={SQL_LANGUAGE_ID}
              value={buffer}
              onChange={(t) => setBuffer("sql", t)}
              onRun={run}
              validate={validateSql}
            />
          </Panel>
          <PanelResizeHandle className="h-px bg-border transition-colors hover:bg-ring/40" />
          <Panel defaultSize={45} minSize={15}>
            <ResultsPane state={state} />
          </Panel>
        </PanelGroup>
        <StatusBar
          dialect="SQL"
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

function toResultsState(resp: SqlExecuteResponse): ResultsState {
  return {
    kind: "table",
    columns: resp.columns.map((c) => ({
      name: c.name,
      typeHint: c.pg_type.toLowerCase(),
    })),
    rows: resp.rows.map((row) =>
      row.map((cell) => (cell as string | number | boolean | null) ?? null)
    ),
    truncated: resp.truncated,
    elapsedMs: resp.elapsed_ms,
  };
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
  return undefined;
}

function elapsedOf(s: ResultsState): number | undefined {
  if (s.kind === "table" || s.kind === "ask" || s.kind === "raw")
    return s.elapsedMs;
  return undefined;
}
