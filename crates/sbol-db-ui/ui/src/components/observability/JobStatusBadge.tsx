import { cn } from "@/lib/utils";
import type { JobStatus } from "@/lib/api";

export function JobStatusBadge({ status }: { status: JobStatus }) {
  const tone =
    status === "succeeded"
      ? "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400"
      : status === "running"
        ? "bg-primary/10 text-primary"
        : status === "queued"
          ? "bg-muted text-muted-foreground"
          : status === "cancelled"
            ? "bg-muted text-muted-foreground"
            : "bg-destructive/10 text-destructive";
  return (
    <span
      className={cn(
        "inline-flex items-center rounded-sm px-1.5 py-0.5 text-[10px] uppercase tracking-wide",
        tone
      )}
    >
      {status}
    </span>
  );
}
