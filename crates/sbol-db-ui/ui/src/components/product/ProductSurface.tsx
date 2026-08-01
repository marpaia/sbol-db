import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

export type SurfaceDensity = "comfortable" | "compact" | "workbench";

const surfaceDensity: Record<SurfaceDensity, string> = {
  comfortable: "rounded-[4px] shadow-[0_1px_0_hsl(var(--foreground)/0.06)]",
  compact: "rounded-[3px]",
  workbench: "rounded-[2px]",
};

const headerDensity: Record<SurfaceDensity, string> = {
  comfortable: "px-5 py-5 sm:px-6",
  compact: "px-4 py-3",
  workbench: "px-3 py-2",
};

const bodyDensity: Record<SurfaceDensity, string> = {
  comfortable: "p-5 sm:p-6",
  compact: "p-4",
  workbench: "p-3",
};

export function ProductSurface({
  density = "comfortable",
  className,
  children,
  id,
}: {
  density?: SurfaceDensity;
  className?: string;
  children: ReactNode;
  id?: string;
}) {
  return (
    <section
      id={id}
      className={cn(
        "overflow-hidden border bg-card text-card-foreground",
        surfaceDensity[density],
        className
      )}
    >
      {children}
    </section>
  );
}

export function ProductSurfaceHeader({
  icon: Icon,
  title,
  description,
  action,
  density = "comfortable",
  className,
}: {
  icon?: LucideIcon;
  title: string;
  description?: string;
  action?: ReactNode;
  density?: SurfaceDensity;
  className?: string;
}) {
  return (
    <header
      className={cn(
        "flex flex-wrap items-start justify-between gap-4 border-b bg-muted/15",
        headerDensity[density],
        className
      )}
    >
      <div className="flex min-w-0 items-start gap-3.5">
        {Icon && (
          <span className="flex size-9 shrink-0 items-center justify-center rounded-[3px] border-l-2 border-primary bg-primary/10 text-primary">
            <Icon className="size-4" aria-hidden="true" />
          </span>
        )}
        <div className="min-w-0">
          <h2 className="text-sm font-semibold tracking-tight">{title}</h2>
          {description && (
            <p
              className={cn(
                "mt-1 max-w-2xl text-muted-foreground",
                density === "comfortable"
                  ? "text-sm leading-6"
                  : "text-xs leading-5"
              )}
            >
              {description}
            </p>
          )}
        </div>
      </div>
      {action}
    </header>
  );
}

export function ProductSurfaceBody({
  density = "comfortable",
  className,
  children,
}: {
  density?: SurfaceDensity;
  className?: string;
  children: ReactNode;
}) {
  return <div className={cn(bodyDensity[density], className)}>{children}</div>;
}
