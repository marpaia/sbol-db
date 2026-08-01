import { CheckCircle2, Clock3, History, XCircle } from "lucide-react";

import { AdminPage, AdminSection } from "@/components/admin/AdminPage";
import { SurfaceState } from "@/components/portal/SurfaceState";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { useAdminAudit } from "@/features/admin/queries";
import { cn } from "@/lib/utils";

export default function AdminAuditRoute() {
  const query = useAdminAudit();

  return (
    <AdminPage
      title="Administrator activity"
      description="Review append-only evidence for native administrator commands. Attempts and terminal outcomes are separate events so failed or interrupted destructive operations remain visible."
    >
      <AdminSection
        title="Event stream"
        description={
          query.data
            ? `${query.data.total} newest events`
            : "Newest events first"
        }
      >
        {query.error ? (
          <SurfaceState
            variant="error"
            title="Activity unavailable"
            description={(query.error as Error).message}
          />
        ) : query.isLoading || !query.data ? (
          <div className="space-y-3">
            <Skeleton className="h-20 rounded-lg" />
            <Skeleton className="h-20 rounded-lg" />
            <Skeleton className="h-20 rounded-lg" />
          </div>
        ) : query.data.items.length === 0 ? (
          <SurfaceState
            icon={History}
            title="No administrator activity"
            description="Native admin mutations will appear here. Compatibility endpoint activity remains in deployment logs until it is migrated."
          />
        ) : (
          <ol className="divide-y rounded-lg border">
            {query.data.items.map((event) => {
              const Icon =
                event.outcome === "succeeded"
                  ? CheckCircle2
                  : event.outcome === "failed"
                    ? XCircle
                    : Clock3;
              return (
                <li
                  key={event.iri}
                  className="flex items-start gap-3 px-4 py-4"
                >
                  <span
                    className={cn(
                      "mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-full",
                      event.outcome === "succeeded" &&
                        "bg-success/10 text-success",
                      event.outcome === "failed" &&
                        "bg-destructive/10 text-destructive",
                      event.outcome === "attempted" &&
                        "bg-muted text-muted-foreground"
                    )}
                  >
                    <Icon className="size-3.5" />
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <code className="text-xs font-semibold">
                        {event.action}
                      </code>
                      <Badge variant="outline">{event.outcome}</Badge>
                      <time
                        className="ml-auto text-[11px] text-muted-foreground"
                        dateTime={event.occurred_at}
                      >
                        {new Date(event.occurred_at).toLocaleString()}
                      </time>
                    </div>
                    <p className="mt-1 break-all text-xs text-muted-foreground">
                      <span className="font-medium text-foreground">
                        {event.actor}
                      </span>
                      {" → "}
                      {event.target}
                    </p>
                    {event.detail && (
                      <p className="mt-1 text-xs leading-5 text-muted-foreground">
                        {event.detail}
                      </p>
                    )}
                  </div>
                </li>
              );
            })}
          </ol>
        )}
      </AdminSection>
    </AdminPage>
  );
}
