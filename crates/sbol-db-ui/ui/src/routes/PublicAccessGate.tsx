import { Navigate, Outlet, useLocation } from "react-router-dom";

import { Skeleton } from "@/components/ui/skeleton";
import { useInstance } from "@/features/instance/queries";
import { useSession } from "@/features/session/queries";

/** Honor the instance-wide `require_login` policy before discovery routes
 * issue their API queries. The V2 middleware enforces the same policy, so this
 * is an ergonomic redirect rather than the security boundary. */
export default function PublicAccessGate() {
  const location = useLocation();
  const instance = useInstance();
  const session = useSession();

  if (instance.isLoading || session.isLoading) {
    return (
      <div className="mx-auto max-w-5xl space-y-4 px-4 py-20 sm:px-6">
        <Skeleton className="mx-auto h-8 w-64" />
        <Skeleton className="mx-auto h-4 w-full max-w-xl" />
        <Skeleton className="mx-auto mt-8 h-14 w-full max-w-3xl rounded-xl" />
      </div>
    );
  }

  if (instance.data?.policies.require_login && !session.data?.authenticated) {
    const next = `${location.pathname}${location.search}`;
    return <Navigate to={`/login?next=${encodeURIComponent(next)}`} replace />;
  }

  return <Outlet />;
}
