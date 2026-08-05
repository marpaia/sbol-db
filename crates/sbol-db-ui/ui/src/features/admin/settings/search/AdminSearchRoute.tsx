import { RefreshCw, Search, Workflow } from "lucide-react";
import { Link } from "react-router-dom";

import {
  AdminPage,
  AdminSection,
  MutationStatus,
} from "@/components/admin/AdminPage";
import { JobStatusBadge } from "@/components/observability/JobStatusBadge";
import { SurfaceState } from "@/components/portal/SurfaceState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import {
  useRebuildSearch,
  useSearchStatus,
} from "@/features/admin/settings/search/queries";
import { adminPath } from "@/lib/routes";

export default function AdminSearchRoute() {
  const query = useSearchStatus();
  const rebuild = useRebuildSearch();

  return (
    <AdminPage
      title="Search indexes"
      description="Inspect the immutable search strategy deployment and schedule a full derived-index rebuild. Ranked text is the development default; embedding strategies appear here only when explicitly configured."
      action={
        <Button onClick={() => rebuild.mutate()} disabled={rebuild.isPending}>
          <RefreshCw className={rebuild.isPending ? "animate-spin" : ""} />
          Rebuild all indexes
        </Button>
      }
    >
      <MutationStatus
        pending={rebuild.isPending}
        error={rebuild.error}
        success={
          rebuild.data
            ? `Rebuild job ${rebuild.data.job.id.slice(0, 8)} queued.`
            : null
        }
      />
      {query.error ? (
        <SurfaceState
          variant="error"
          title="Search status unavailable"
          description={(query.error as Error).message}
        />
      ) : query.isLoading || !query.data ? (
        <div className="grid gap-4 md:grid-cols-2">
          <Skeleton className="h-48 rounded-xl" />
          <Skeleton className="h-48 rounded-xl" />
        </div>
      ) : (
        <>
          <AdminSection
            title="Configured strategies"
            description={`${query.data.strategies.length} strategy${query.data.strategies.length === 1 ? "" : "ies"} available to the native search contract.`}
          >
            <div className="grid gap-3 md:grid-cols-2">
              {query.data.strategies.map((strategy) => (
                <article
                  key={strategy.id}
                  className="rounded-lg border bg-background p-4"
                >
                  <div className="flex items-start gap-3">
                    <span className="flex size-9 items-center justify-center rounded-lg bg-primary/10 text-primary">
                      {strategy.id === query.data.default_strategy ? (
                        <Search className="size-4" />
                      ) : (
                        <Workflow className="size-4" />
                      )}
                    </span>
                    <div className="min-w-0 flex-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <h3 className="font-medium">
                          {strategy.label || strategy.id}
                        </h3>
                        {strategy.id === query.data.default_strategy && (
                          <Badge>Default</Badge>
                        )}
                      </div>
                      <code className="mt-1 block text-[11px] text-muted-foreground">
                        {strategy.id}
                      </code>
                      {strategy.description && (
                        <p className="mt-3 text-xs leading-5 text-muted-foreground">
                          {strategy.description}
                        </p>
                      )}
                    </div>
                  </div>
                </article>
              ))}
            </div>
          </AdminSection>

          <AdminSection
            title="Recent rebuilds"
            description="The worker rebuilds text, topology, and configured vector indexes as one tracked operation."
          >
            {!query.data.recent_rebuilds?.length ? (
              <SurfaceState
                title="No rebuild jobs"
                description="Schedule a rebuild after importing a large archive or changing search deployment configuration."
              />
            ) : (
              <div className="divide-y rounded-lg border">
                {query.data.recent_rebuilds.map((job) => (
                  <Link
                    key={job.id}
                    to={adminPath(`/observability/jobs/${job.id}`)}
                    className="flex flex-wrap items-center gap-3 px-4 py-3 transition-colors hover:bg-muted/30"
                  >
                    <JobStatusBadge status={job.status} />
                    <code className="text-xs">{job.id.slice(0, 8)}</code>
                    <span className="text-xs text-muted-foreground">
                      {new Date(job.created_at).toLocaleString()}
                    </span>
                    <span className="ml-auto text-xs text-muted-foreground">
                      {job.attempts}/{job.max_attempts} attempts
                    </span>
                  </Link>
                ))}
              </div>
            )}
          </AdminSection>
        </>
      )}
    </AdminPage>
  );
}
