import { Badge } from "@/components/ui/badge";
import type { ObjectContentState } from "@/features/registry/objects/api";
import { cn } from "@/lib/utils";

const labels: Record<ObjectContentState, string> = {
  available: "Available",
  empty: "None asserted",
  partial: "Partial",
  unsupported: "Not applicable",
};

export function ObjectStateBadge({
  state,
  className,
}: {
  state: ObjectContentState;
  className?: string;
}) {
  return (
    <Badge
      variant="outline"
      className={cn(
        "whitespace-nowrap text-[10px]",
        state === "available" &&
          "border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
        state === "partial" &&
          "border-amber-500/25 bg-amber-500/10 text-amber-700 dark:text-amber-300",
        state === "unsupported" && "border-dashed text-muted-foreground",
        state === "empty" && "bg-muted/40 text-muted-foreground",
        className
      )}
    >
      {labels[state]}
    </Badge>
  );
}
