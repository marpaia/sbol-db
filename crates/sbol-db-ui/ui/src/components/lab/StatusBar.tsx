import { Download } from "lucide-react";

import { cn } from "@/lib/utils";

export interface StatusBarProps {
  rowCount?: number;
  elapsedMs?: number;
  truncated?: boolean;
  dialect: string;
  status: "idle" | "running" | "ok" | "error";
  /** When present, exposes CSV / JSON download buttons that call the
   *  provided callbacks. Omit to hide the export controls (e.g., when
   *  no results are loaded). */
  onExportCsv?: () => void;
  onExportJson?: () => void;
}

export function StatusBar({
  rowCount,
  elapsedMs,
  truncated,
  dialect,
  status,
  onExportCsv,
  onExportJson,
}: StatusBarProps) {
  return (
    <footer className="border-t bg-background text-xs">
      <div className="mx-auto flex max-w-full items-center gap-4 px-4 py-1.5 text-muted-foreground">
        <StatusDot status={status} />
        <span className="text-[10px] uppercase tracking-wider">{dialect}</span>
        <span className="ml-auto flex items-center gap-4">
          {(onExportCsv || onExportJson) && (
            <span className="flex items-center gap-1">
              {onExportCsv && (
                <button
                  type="button"
                  onClick={onExportCsv}
                  className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                  title="Download CSV"
                >
                  <Download size={12} />
                  CSV
                </button>
              )}
              {onExportJson && (
                <button
                  type="button"
                  onClick={onExportJson}
                  className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                  title="Download JSON"
                >
                  <Download size={12} />
                  JSON
                </button>
              )}
            </span>
          )}
          {typeof rowCount === "number" && (
            <span>
              <span className="text-muted-foreground/70">rows </span>
              <span className="tabular-nums text-foreground">
                {rowCount.toLocaleString()}
              </span>
              {truncated && (
                <span className="ml-1 text-warning">(truncated)</span>
              )}
            </span>
          )}
          {typeof elapsedMs === "number" && (
            <span>
              <span className="text-muted-foreground/70">time </span>
              <span className="tabular-nums text-foreground">
                {elapsedMs} ms
              </span>
            </span>
          )}
        </span>
      </div>
    </footer>
  );
}

function StatusDot({ status }: { status: StatusBarProps["status"] }) {
  const color = {
    idle: "bg-muted-foreground/40",
    running: "bg-foreground animate-pulse",
    ok: "bg-success",
    error: "bg-destructive",
  }[status];
  return (
    <span
      aria-label={status}
      className={cn("inline-block h-1.5 w-1.5 rounded-full", color)}
    />
  );
}
