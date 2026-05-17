/**
 * KPI tile for the observability page. Matches the dashboard's
 * `CountCard` aesthetic but adds a sublabel slot for units and a
 * loading state that shows an em-dash placeholder.
 */

import { cn } from "@/lib/utils";

interface KpiTileProps {
  label: string;
  value: string | number | undefined;
  unit?: string;
  hint?: string;
  loading?: boolean;
  tone?: "default" | "warn" | "error" | "ok";
}

export function KpiTile({
  label,
  value,
  unit,
  hint,
  loading,
  tone = "default",
}: KpiTileProps) {
  const formatted =
    loading || value === undefined || value === null
      ? "—"
      : typeof value === "number"
        ? value.toLocaleString()
        : value;

  return (
    <div className="rounded-lg border bg-card p-4">
      <div className="text-[11px] font-medium uppercase tracking-wider text-muted-foreground">
        {label}
      </div>
      <div className="mt-2 flex items-baseline gap-1.5">
        <span
          className={cn(
            "text-2xl font-semibold tabular-nums",
            loading || value === undefined
              ? "text-muted-foreground/40"
              : tone === "warn"
                ? "text-amber-500"
                : tone === "error"
                  ? "text-destructive"
                  : tone === "ok"
                    ? "text-emerald-500"
                    : "text-foreground"
          )}
        >
          {formatted}
        </span>
        {unit && !loading && value !== undefined && (
          <span className="text-xs text-muted-foreground">{unit}</span>
        )}
      </div>
      {hint && (
        <div className="mt-1 truncate text-[11px] text-muted-foreground/70">
          {hint}
        </div>
      )}
    </div>
  );
}
