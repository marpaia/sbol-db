import { CheckCircle2, Loader2, TriangleAlert } from "lucide-react";
import type { ReactNode } from "react";

import {
  ProductSurface,
  ProductSurfaceBody,
  ProductSurfaceHeader,
} from "@/components/product/ProductSurface";
import { cn } from "@/lib/utils";

export function AdminPage({
  title,
  description,
  eyebrow = "Admin workspace",
  action,
  children,
  maxWidth = "6xl",
}: {
  title: string;
  description: string;
  eyebrow?: string;
  action?: ReactNode;
  children: ReactNode;
  maxWidth?: "5xl" | "6xl" | "7xl";
}) {
  return (
    <div className="h-full w-full overflow-y-auto">
      <div
        className={cn(
          "mx-auto space-y-8 px-5 py-8 sm:px-8 sm:py-10",
          maxWidth === "5xl"
            ? "max-w-5xl"
            : maxWidth === "7xl"
              ? "max-w-7xl"
              : "max-w-6xl"
        )}
      >
        <header className="flex flex-wrap items-start justify-between gap-4">
          <div className="max-w-2xl">
            <p className="mb-2 text-[11px] font-medium uppercase tracking-[0.16em] text-primary">
              {eyebrow}
            </p>
            <h1 className="text-2xl font-semibold tracking-tight">{title}</h1>
            <p className="mt-2 text-sm leading-6 text-muted-foreground">
              {description}
            </p>
          </div>
          {action}
        </header>
        {children}
      </div>
    </div>
  );
}

export function AdminSection({
  title,
  description,
  action,
  children,
}: {
  title: string;
  description?: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <ProductSurface density="compact">
      <ProductSurfaceHeader
        density="compact"
        title={title}
        description={description}
        action={action}
      />
      <ProductSurfaceBody density="compact">{children}</ProductSurfaceBody>
    </ProductSurface>
  );
}

export function MutationStatus({
  pending,
  error,
  success,
}: {
  pending?: boolean;
  error?: unknown;
  success?: string | null;
}) {
  if (pending) {
    return (
      <p
        className="flex items-center gap-2 text-xs text-muted-foreground"
        role="status"
      >
        <Loader2 className="size-3.5 animate-spin" /> Saving changes…
      </p>
    );
  }
  if (error) {
    return (
      <p
        className="flex items-start gap-2 text-xs leading-5 text-destructive"
        role="alert"
      >
        <TriangleAlert className="mt-0.5 size-3.5 shrink-0" />
        {error instanceof Error ? error.message : "The operation failed."}
      </p>
    );
  }
  if (success) {
    return (
      <p className="flex items-center gap-2 text-xs text-success" role="status">
        <CheckCircle2 className="size-3.5" /> {success}
      </p>
    );
  }
  return null;
}

export function Field({
  label,
  hint,
  children,
  className,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <label className={cn("grid gap-1.5", className)}>
      <span className="text-xs font-medium">{label}</span>
      {children}
      {hint && (
        <span className="text-[11px] leading-4 text-muted-foreground">
          {hint}
        </span>
      )}
    </label>
  );
}
