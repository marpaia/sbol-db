/**
 * History panel: scrolling list of the last N runs (Zustand-backed).
 * Click an entry to load it back into the editor. Failed runs get a
 * subtle red dot so the user can tell at a glance whether a query
 * was successful.
 */

import { History, Trash2 } from "lucide-react";

import { type Dialect, useLabStore } from "@/lib/store";
import { cn, compactQuery } from "@/lib/utils";

export interface QueryHistoryListProps {
  dialect: Dialect;
  onLoad: (query: string) => void;
}

export function QueryHistoryList({ dialect, onLoad }: QueryHistoryListProps) {
  const history = useLabStore((s) =>
    s.history.filter((h) => h.dialect === dialect)
  );
  const clearHistory = useLabStore((s) => s.clearHistory);

  return (
    <div className="flex h-full w-full flex-col bg-background">
      <div className="flex items-center gap-2 border-b px-3 py-2 text-[11px] uppercase tracking-wider text-muted-foreground">
        <History size={12} /> History
        {history.length > 0 && (
          <button
            type="button"
            onClick={clearHistory}
            className="ml-auto text-muted-foreground/60 transition-colors hover:text-foreground"
            aria-label="Clear history"
            title="Clear history"
          >
            <Trash2 size={12} />
          </button>
        )}
      </div>
      <ul className="flex-1 overflow-y-auto">
        {history.length === 0 && (
          <li className="px-3 py-2 text-xs text-muted-foreground">
            No history yet.
          </li>
        )}
        {history.map((h) => (
          <li key={h.id} className="border-b">
            <button
              type="button"
              onClick={() => onLoad(h.query)}
              className="w-full px-3 py-1.5 text-left transition-colors hover:bg-accent"
              title="Click to load"
            >
              <div className="flex items-center gap-2 text-[10px] text-muted-foreground">
                <span
                  className={cn(
                    "inline-block h-1.5 w-1.5 rounded-full",
                    h.ok ? "bg-success" : "bg-destructive"
                  )}
                  aria-hidden
                />
                <span className="tabular-nums">{formatTime(h.ranAt)}</span>
                <span className="ml-auto tabular-nums">
                  {h.elapsedMs} ms · {h.rowCount} rows
                </span>
              </div>
              <div className="mt-0.5 truncate font-mono text-xs text-foreground/90">
                {compactQuery(h.query) || "(empty)"}
              </div>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

function formatTime(epochMs: number): string {
  const d = new Date(epochMs);
  return d.toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}
