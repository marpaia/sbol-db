import { Boxes } from "lucide-react";
import { Link, useParams } from "react-router-dom";

import {
  MachineAccess,
  ObjectDownloads,
} from "@/components/portal/ObjectDownloads";
import { CollectionManagement } from "@/components/portal/CollectionManagement";
import { ObjectAttachments } from "@/components/portal/ObjectAttachments";
import { ObjectBiology } from "@/components/portal/ObjectBiology";
import { ObjectContext } from "@/components/portal/ObjectContext";
import { ObjectCollaboration } from "@/components/portal/ObjectCollaboration";
import { ObjectHeader } from "@/components/portal/ObjectHeader";
import { ObjectIdentity } from "@/components/portal/ObjectIdentity";
import { ObjectPropertyBrowser } from "@/components/portal/ObjectPropertyBrowser";
import { ObjectProvenance } from "@/components/portal/ObjectProvenance";
import { ObjectRawProjection } from "@/components/portal/ObjectRawProjection";
import { ObjectReview } from "@/components/portal/ObjectReview";
import { ObjectVisualFallback } from "@/components/portal/ObjectVisualFallback";
import { SimilarDesigns } from "@/components/portal/SimilarDesigns";
import { SurfaceState } from "@/components/portal/SurfaceState";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { PortalApiError } from "@/features/portal/api";
import { usePortalObjectDetails } from "@/features/portal/queries";

export default function PublicObjectRoute() {
  const params = useParams<{ iri: string }>();
  const iri = decodeURIComponent(params.iri || "");
  const object = usePortalObjectDetails(iri);

  if (object.isLoading) return <ObjectPageSkeleton />;

  if (object.error || !object.data) {
    const missing =
      object.error instanceof PortalApiError && object.error.status === 404;
    return (
      <div className="mx-auto max-w-3xl px-4 py-20 sm:px-6">
        <SurfaceState
          variant={missing ? "empty" : "error"}
          icon={Boxes}
          title={missing ? "Design not found" : "Couldn’t load this design"}
          description={
            missing
              ? "It may not exist, or it may be outside the graphs visible to your account."
              : (object.error as Error)?.message ||
                "The object resource could not be read."
          }
          action={
            <Button asChild variant="outline" size="sm">
              <Link to="/search">Return to search</Link>
            </Button>
          }
        />
      </div>
    );
  }

  const data = object.data;
  return (
    <>
      <ObjectHeader object={data} />
      <div className="mx-auto max-w-7xl px-4 py-8 sm:px-6 sm:py-10 lg:px-8">
        <div className="grid items-start gap-6 lg:grid-cols-[minmax(0,1fr)_20rem] xl:gap-8">
          <div className="min-w-0 space-y-6">
            <ObjectIdentity object={data} />
            <ObjectProvenance object={data} />
            <ObjectVisualFallback object={data} />
            <ObjectBiology object={data} />
            <ObjectContext object={data} />
            <ObjectAttachments object={data} />
            <ObjectPropertyBrowser object={data} />
            <SimilarDesigns iri={data.iri} />
            <ObjectRawProjection object={data} />
          </div>

          <aside
            className="space-y-4 lg:sticky lg:top-24"
            aria-label="Object actions"
          >
            <ObjectCollaboration object={data} />
            <ObjectReview object={data} />
            <CollectionManagement object={data} />
            <ObjectDownloads object={data} />
            <MachineAccess iri={data.iri} />
          </aside>
        </div>
      </div>
    </>
  );
}

function ObjectPageSkeleton() {
  return (
    <div aria-label="Loading object">
      <div className="border-b bg-muted/10">
        <div className="mx-auto max-w-7xl space-y-5 px-4 py-9 sm:px-6 lg:px-8">
          <Skeleton className="h-5 w-28" />
          <div className="flex gap-4">
            <Skeleton className="size-14 shrink-0 rounded-xl" />
            <div className="w-full max-w-3xl space-y-3">
              <Skeleton className="h-8 w-2/3" />
              <Skeleton className="h-5 w-full" />
              <Skeleton className="h-11 w-full rounded-lg" />
            </div>
          </div>
        </div>
      </div>
      <div className="mx-auto grid max-w-7xl gap-6 px-4 py-8 sm:px-6 lg:grid-cols-[minmax(0,1fr)_20rem] lg:px-8">
        <div className="space-y-6">
          <Skeleton className="h-80 rounded-xl" />
          <Skeleton className="h-64 rounded-xl" />
          <Skeleton className="h-96 rounded-xl" />
        </div>
        <Skeleton className="h-[34rem] rounded-xl" />
      </div>
    </div>
  );
}
