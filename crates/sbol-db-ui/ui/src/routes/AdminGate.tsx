import { AlertTriangle, ArrowLeft } from "lucide-react";
import { Link, Navigate, Outlet, useLocation } from "react-router-dom";

import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { useInstance, useSession } from "@/features/portal/queries";

export default function AdminGate() {
  const location = useLocation();
  const instance = useInstance();
  const session = useSession();

  if (instance.isLoading || session.isLoading) {
    return (
      <div className="flex min-h-svh items-center justify-center bg-background">
        <div className="w-full max-w-sm space-y-3 px-6">
          <Skeleton className="h-10 w-10 rounded-xl" />
          <Skeleton className="h-7 w-48" />
          <Skeleton className="h-4 w-full" />
        </div>
      </div>
    );
  }

  if (instance.data?.setup_required) return <Navigate to="/setup" replace />;

  if (!session.data?.authenticated || !session.data.user) {
    const next = `${location.pathname}${location.search}`;
    return <Navigate to={`/login?next=${encodeURIComponent(next)}`} replace />;
  }

  if (!session.data.user.is_admin) {
    return (
      <div className="flex min-h-svh items-center justify-center bg-muted/20 px-6">
        <div className="max-w-md rounded-xl border bg-card p-8 text-center shadow-sm">
          <span className="mx-auto flex size-11 items-center justify-center rounded-xl bg-warning/10 text-warning">
            <AlertTriangle className="size-5" />
          </span>
          <h1 className="mt-5 text-xl font-semibold">
            Administrator access required
          </h1>
          <p className="mt-2 text-sm leading-6 text-muted-foreground">
            Your account is signed in, but it does not have permission to open
            the data and operations workspace.
          </p>
          <Button asChild variant="outline" className="mt-6">
            <Link to="/">
              <ArrowLeft /> Return to the registry
            </Link>
          </Button>
        </div>
      </div>
    );
  }

  return <Outlet />;
}
