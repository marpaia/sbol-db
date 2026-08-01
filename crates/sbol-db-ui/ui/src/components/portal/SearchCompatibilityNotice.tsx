import { AlertTriangle } from "lucide-react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

export function SearchCompatibilityNotice({
  warnings,
  onDismiss,
}: {
  warnings: string[];
  onDismiss: () => void;
}) {
  const hasWarnings = warnings.length > 0;

  return (
    <div
      className={cn(
        "mt-6 flex items-start gap-3 rounded-xl border p-4 text-sm",
        hasWarnings
          ? "border-amber-500/30 bg-amber-500/5"
          : "border-primary/20 bg-primary/5"
      )}
      role={hasWarnings ? "alert" : "status"}
    >
      <AlertTriangle
        className={cn(
          "mt-0.5 size-4 shrink-0",
          hasWarnings ? "text-amber-600 dark:text-amber-400" : "text-primary"
        )}
      />
      <div className="min-w-0 flex-1">
        <p className="font-medium">
          {hasWarnings
            ? "This compatibility link was only partly translated"
            : "Compatibility link translated to this search"}
        </p>
        {hasWarnings ? (
          <ul className="mt-1 list-disc space-y-1 pl-4 text-xs leading-5 text-muted-foreground">
            {warnings.map((warning) => (
              <li key={warning}>{warning}</li>
            ))}
          </ul>
        ) : (
          <p className="mt-1 text-xs leading-5 text-muted-foreground">
            Its supported fields now live in this shareable SBOL DB URL.
          </p>
        )}
      </div>
      <Button variant="ghost" size="sm" onClick={onDismiss}>
        Dismiss
      </Button>
    </div>
  );
}
