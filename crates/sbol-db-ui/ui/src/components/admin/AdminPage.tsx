import { CheckCircle2, Loader2, TriangleAlert } from "lucide-react";
import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

export function AdminPage({
  title,
  description,
  action,
  children,
}: {
  title: string;
  description: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-[90rem] space-y-8 px-5 py-8 sm:px-8 sm:py-10">
        <header className="flex flex-wrap items-start justify-between gap-4 border-b border-foreground/15 pb-6">
          <div className="max-w-2xl">
            <p className="ledger-label text-primary">Admin control plane</p>
            <h1 className="mt-2 text-3xl font-semibold tracking-[-0.025em]">
              {title}
            </h1>
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
    <section className="border border-foreground/15 bg-card/80">
      <header className="flex flex-wrap items-start justify-between gap-4 border-b border-foreground/15 bg-muted/10 px-5 py-4 sm:px-6">
        <div>
          <h2 className="text-sm font-semibold">{title}</h2>
          {description && (
            <p className="mt-1 max-w-2xl text-xs leading-5 text-muted-foreground">
              {description}
            </p>
          )}
        </div>
        {action}
      </header>
      <div className="p-5 sm:p-6">{children}</div>
    </section>
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
