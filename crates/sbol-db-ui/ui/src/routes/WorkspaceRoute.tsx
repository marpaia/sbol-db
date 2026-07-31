import {
  ClipboardCheck,
  FilePlus2,
  FolderKanban,
  LockKeyhole,
  RefreshCw,
  UsersRound,
} from "lucide-react";
import { Link, useLocation } from "react-router-dom";

import { ObjectResultCard } from "@/components/portal/ObjectResultCard";
import { SurfaceState } from "@/components/portal/SurfaceState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import type {
  PortalObjectDetails,
  PortalSearchHit,
  ReviewCase,
} from "@/features/portal/api";
import {
  usePortalSearch,
  useReviews,
  useSession,
  useSharedObjects,
} from "@/features/portal/queries";
import { publicObjectPath } from "@/lib/routes";

const COLLECTION = "http://sbols.org/v2#Collection";

export default function WorkspaceRoute() {
  const session = useSession();
  const location = useLocation();

  if (session.isLoading) return <WorkspaceSkeleton />;
  if (!session.data?.authenticated || !session.data.user) {
    const next = `${location.pathname}${location.search}`;
    return (
      <div className="mx-auto max-w-3xl px-4 py-20 sm:px-6">
        <SurfaceState
          variant="info"
          icon={LockKeyhole}
          title="Sign in to open your workspace"
          description="Your workspace keeps owned contributions and read-only collaborations distinct."
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

  const view = location.pathname.endsWith("/shared")
    ? "shared"
    : location.pathname.endsWith("/reviews")
      ? "reviews"
      : "owned";
  return (
    <div className="mx-auto max-w-7xl px-4 py-10 sm:px-6 lg:px-8">
      <header className="flex flex-col gap-5 border-b pb-8 sm:flex-row sm:items-end sm:justify-between">
        <div>
          <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-[0.14em] text-primary">
            <FolderKanban className="size-3.5" aria-hidden="true" /> Account
            workspace
          </div>
          <h1 className="mt-3 text-3xl font-semibold tracking-tight">
            {session.data.user.name}&rsquo;s workspace
          </h1>
          <p className="mt-3 max-w-2xl text-sm leading-6 text-muted-foreground">
            Owned collections remain writable. Shared objects are read-only
            unless their owner explicitly transfers ownership.
          </p>
        </div>
        <Button asChild>
          <Link to="/contribute">
            <FilePlus2 /> New contribution
          </Link>
        </Button>
      </header>

      <nav aria-label="Workspace views" className="mt-6 flex gap-2">
        <Button
          asChild
          variant={view === "owned" ? "secondary" : "ghost"}
          size="sm"
        >
          <Link to="/workspace">
            <FolderKanban /> Owned
          </Link>
        </Button>
        <Button
          asChild
          variant={view === "shared" ? "secondary" : "ghost"}
          size="sm"
        >
          <Link to="/workspace/shared">
            <UsersRound /> Shared with me
          </Link>
        </Button>
        <Button
          asChild
          variant={view === "reviews" ? "secondary" : "ghost"}
          size="sm"
        >
          <Link to="/workspace/reviews">
            <ClipboardCheck /> Reviews
          </Link>
        </Button>
      </nav>

      <div className="mt-6">
        {view === "shared" ? (
          <SharedObjects />
        ) : view === "reviews" ? (
          <ReviewQueue userGraph={session.data.user.graph_uri} />
        ) : (
          <OwnedCollections owner={session.data.user.graph_uri} />
        )}
      </div>
    </div>
  );
}

function ReviewQueue({ userGraph }: { userGraph: string }) {
  const reviews = useReviews();
  if (reviews.isLoading) return <CollectionGridSkeleton />;
  if (reviews.error) {
    return (
      <SurfaceState
        variant="error"
        title="Couldn’t load reviews"
        description={reviews.error.message}
        action={
          <Button variant="outline" size="sm" onClick={() => reviews.refetch()}>
            <RefreshCw /> Try again
          </Button>
        }
      />
    );
  }
  if (!reviews.data?.items.length) {
    return (
      <SurfaceState
        icon={ClipboardCheck}
        title="No review work yet"
        description="Review requests you submit or that a curator assigns to you will appear here."
      />
    );
  }
  return (
    <div className="space-y-4">
      <p className="text-xs text-muted-foreground">
        {reviews.data.total.toLocaleString()} active or completed review
        {reviews.data.total === 1 ? "" : "s"}
      </p>
      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
        {reviews.data.items.map((review) => (
          <ReviewQueueCard
            key={review.object_iri}
            review={review}
            userGraph={userGraph}
          />
        ))}
      </div>
    </div>
  );
}

function ReviewQueueCard({
  review,
  userGraph,
}: {
  review: ReviewCase;
  userGraph: string;
}) {
  const assigned = review.curator_graph === userGraph;
  const status =
    review.status === "pending"
      ? "Pending"
      : review.status === "approved"
        ? "Approved"
        : "Changes requested";
  return (
    <Link
      to={publicObjectPath(review.object_iri)}
      className="group rounded-xl border bg-card p-5 shadow-sm transition-[border-color,box-shadow] duration-200 motion-reduce:transition-none hover:border-primary/35 hover:shadow-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
    >
      <div className="flex items-start justify-between gap-3">
        <span className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
          <ClipboardCheck className="size-4" aria-hidden="true" />
        </span>
        <Badge variant="outline">{status}</Badge>
      </div>
      <p className="mt-4 break-all text-sm font-semibold leading-5 group-hover:text-primary">
        {review.object_iri}
      </p>
      <p className="mt-2 text-xs text-muted-foreground">
        {assigned ? "Assigned to you" : "Requested by you"} · Updated{" "}
        {new Intl.DateTimeFormat(undefined, { dateStyle: "medium" }).format(
          new Date(review.updated_at)
        )}
      </p>
      {review.note && (
        <p className="mt-3 line-clamp-2 text-xs leading-5 text-muted-foreground">
          {review.note}
        </p>
      )}
    </Link>
  );
}

function OwnedCollections({ owner }: { owner: string }) {
  const collections = usePortalSearch({
    owner,
    type: COLLECTION,
    sort: "modified",
    direction: "desc",
    limit: 100,
  });

  if (collections.isLoading) return <CollectionGridSkeleton />;
  if (collections.error) {
    return (
      <SurfaceState
        variant="error"
        title="Couldn’t load your collections"
        description={collections.error.message}
        action={
          <Button
            variant="outline"
            size="sm"
            onClick={() => collections.refetch()}
          >
            <RefreshCw /> Try again
          </Button>
        }
      />
    );
  }
  if (!collections.data?.items.length) {
    return (
      <SurfaceState
        icon={FolderKanban}
        title="No collections yet"
        description="Start with a write-free validation preview. You’ll see every minted identity and conflict before committing."
        action={
          <Button asChild>
            <Link to="/contribute">
              <FilePlus2 /> Contribute your first collection
            </Link>
          </Button>
        }
      />
    );
  }
  return (
    <ObjectGrid
      items={collections.data.items}
      total={collections.data.total}
      capped={collections.data.total > collections.data.items.length}
      noun="owned collection"
    />
  );
}

function SharedObjects() {
  const shared = useSharedObjects();
  if (shared.isLoading) return <CollectionGridSkeleton />;
  if (shared.error) {
    return (
      <SurfaceState
        variant="error"
        title="Couldn’t load shared objects"
        description={shared.error.message}
        action={
          <Button variant="outline" size="sm" onClick={() => shared.refetch()}>
            <RefreshCw /> Try again
          </Button>
        }
      />
    );
  }
  if (!shared.data?.items.length) {
    return (
      <SurfaceState
        icon={UsersRound}
        title="Nothing has been shared with you"
        description="An owner can grant your account read-only access from the collaboration section of a private object."
      />
    );
  }
  return (
    <ObjectGrid
      items={shared.data.items.map(detailsToHit)}
      total={shared.data.total}
      noun="shared object"
    />
  );
}

function ObjectGrid({
  items,
  total,
  capped = false,
  noun,
}: {
  items: PortalSearchHit[];
  total: number;
  capped?: boolean;
  noun: string;
}) {
  return (
    <>
      <div className="mb-4 flex items-center justify-between gap-4 text-xs text-muted-foreground">
        <span>
          {total.toLocaleString()} {noun}
          {total === 1 ? "" : "s"}
        </span>
        {capped && <span>Showing the 100 most recently modified</span>}
      </div>
      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
        {items.map((item) => (
          <ObjectResultCard key={item.uri} hit={item} />
        ))}
      </div>
    </>
  );
}

function detailsToHit(details: PortalObjectDetails): PortalSearchHit {
  return {
    uri: details.iri,
    display_id: details.display_id,
    version: details.version,
    name: details.name,
    description: details.description,
    object_type: details.object_type,
    roles: details.roles,
    owners: details.owners,
    created_at: details.created_at,
    modified_at: details.modified_at,
    score: 1,
  };
}

function CollectionGridSkeleton() {
  return (
    <div
      className="grid gap-4 md:grid-cols-2 xl:grid-cols-3"
      aria-label="Loading collections"
    >
      {Array.from({ length: 6 }, (_, index) => (
        <Skeleton key={index} className="h-52 rounded-xl" />
      ))}
    </div>
  );
}

function WorkspaceSkeleton() {
  return (
    <div
      className="mx-auto max-w-7xl space-y-6 px-4 py-10 sm:px-6 lg:px-8"
      aria-label="Loading workspace"
    >
      <Skeleton className="h-4 w-36" />
      <Skeleton className="h-10 w-72" />
      <CollectionGridSkeleton />
    </div>
  );
}
