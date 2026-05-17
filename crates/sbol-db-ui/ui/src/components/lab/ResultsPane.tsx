/**
 * Wraps `ResultsTable` with the empty/loading/error/raw views needed by
 * a query workbench. Accepts already-shaped column/row data; callers
 * (SqlRoute, SparqlRoute) handle the dialect-specific reshape.
 */

import { Inbox, Loader2 } from "lucide-react";

import {
  ResultsTable,
  type ResultColumn,
  type ResultRow,
} from "./ResultsTable";
import { ErrorBanner } from "./ErrorBanner";
import { cn } from "@/lib/utils";

export type ResultsState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "error"; message: string; detail?: string }
  | { kind: "ask"; value: boolean; elapsedMs: number }
  | { kind: "raw"; mime: string; body: string; elapsedMs: number }
  | {
      kind: "table";
      columns: ResultColumn[];
      rows: ResultRow[];
      truncated?: boolean;
      elapsedMs: number;
    };

export function ResultsPane({ state }: { state: ResultsState }) {
  switch (state.kind) {
    case "idle":
      return (
        <Empty
          icon={<Inbox className="size-7 text-muted-foreground/40" />}
          title="No results yet"
          subtitle="Press ⌘↵ to run the query."
        />
      );
    case "loading":
      return (
        <Empty
          icon={<Loader2 className="size-7 animate-spin text-foreground" />}
          title="Running…"
          subtitle="Cancel with the Stop button or by editing the query."
        />
      );
    case "error":
      return <ErrorBanner title={state.message} body={state.detail} />;
    case "ask":
      return (
        <Empty
          icon={
            <span
              className={cn(
                "font-mono text-2xl font-semibold",
                state.value ? "text-success" : "text-destructive"
              )}
            >
              {state.value ? "TRUE" : "FALSE"}
            </span>
          }
          title="ASK result"
          subtitle={`${state.elapsedMs} ms`}
        />
      );
    case "raw":
      return (
        <pre className="h-full w-full overflow-auto border-t bg-background p-4 font-mono text-xs text-foreground/90 whitespace-pre-wrap">
          {state.body}
        </pre>
      );
    case "table":
      if (state.rows.length === 0) {
        return (
          <Empty
            icon={<Inbox className="size-7 text-muted-foreground/40" />}
            title="0 rows"
            subtitle={`${state.elapsedMs} ms`}
          />
        );
      }
      return <ResultsTable columns={state.columns} rows={state.rows} />;
  }
}

function Empty({
  icon,
  title,
  subtitle,
}: {
  icon: React.ReactNode;
  title: string;
  subtitle?: string;
}) {
  return (
    <div className="flex h-full w-full flex-col items-center justify-center gap-2 border-t bg-background text-center">
      {icon}
      <div className="text-sm text-foreground">{title}</div>
      {subtitle && (
        <div className="text-xs text-muted-foreground">{subtitle}</div>
      )}
    </div>
  );
}
