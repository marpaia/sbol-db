import { LockKeyhole } from "lucide-react";
import { Link, useLocation } from "react-router-dom";

import { SurfaceState } from "@/components/portal/SurfaceState";
import { Button } from "@/components/ui/button";
import { useSession } from "@/features/session/queries";
import { ContributionWorkspace } from "./ContributionWorkspace";

/** Enforce contribution access before loading the stateful editor. */
export default function ContributionRoute() {
  const session = useSession();
  const location = useLocation();

  if (session.isLoading) {
    return (
      <div className="mx-auto max-w-3xl px-4 py-20 sm:px-6">
        <SurfaceState
          variant="info"
          title="Loading your workspace"
          description="Checking the active account before opening contribution tools."
        />
      </div>
    );
  }

  if (!session.data?.authenticated || !session.data.user) {
    const next = `${location.pathname}${location.search}`;
    return (
      <div className="mx-auto max-w-3xl px-4 py-20 sm:px-6">
        <SurfaceState
          variant="info"
          icon={LockKeyhole}
          title="Sign in to contribute"
          description="Validation previews are private to your account and never write until you explicitly commit."
          action={
            <Button asChild>
              <Link to={`/login?next=${encodeURIComponent(next)}`}>
                Sign in
              </Link>
            </Button>
          }
        />
      </div>
    );
  }

  if (!session.data.user.is_member && !session.data.user.is_admin) {
    return (
      <div className="mx-auto max-w-3xl px-4 py-20 sm:px-6">
        <SurfaceState
          variant="unsupported"
          icon={LockKeyhole}
          title="Membership is required"
          description="Your account can browse this registry, but an active member role is required to create a collection."
        />
      </div>
    );
  }

  return <ContributionWorkspace />;
}
