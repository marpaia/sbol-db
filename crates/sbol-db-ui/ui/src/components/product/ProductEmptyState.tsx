import type { LucideIcon } from "lucide-react";
import { CircleOff, Info, TriangleAlert } from "lucide-react";
import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

export type EmptyStateVariant = "empty" | "unsupported" | "error" | "info";

export function ProductEmptyState({
  title,
  description,
  variant = "empty",
  icon,
  action,
  density = "comfortable",
  className,
}: {
  title: string;
  description: string;
  variant?: EmptyStateVariant;
  icon?: LucideIcon;
  action?: ReactNode;
  density?: "comfortable" | "compact";
  className?: string;
}) {
  const Icon = icon || stateIcon(variant);
  return (
    <div
      className={cn(
        "rounded-xl border border-dashed text-center",
        density === "comfortable" ? "px-5 py-8" : "px-4 py-6",
        variant === "error"
          ? "border-destructive/25 bg-destructive/5"
          : "bg-muted/10",
        className
      )}
      role={variant === "error" ? "alert" : undefined}
    >
      <span
        className={cn(
          "mx-auto flex size-9 items-center justify-center rounded-full",
          variant === "error"
            ? "bg-destructive/10 text-destructive"
            : "bg-muted text-muted-foreground"
        )}
      >
        <Icon className="size-4" aria-hidden="true" />
      </span>
      <p className="mt-3 text-sm font-medium">{title}</p>
      <p className="mx-auto mt-1 max-w-md text-xs leading-5 text-muted-foreground">
        {description}
      </p>
      {action && <div className="mt-4 flex justify-center">{action}</div>}
    </div>
  );
}

function stateIcon(variant: EmptyStateVariant) {
  if (variant === "error") return TriangleAlert;
  if (variant === "unsupported") return CircleOff;
  return Info;
}
