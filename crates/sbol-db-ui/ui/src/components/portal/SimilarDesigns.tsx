import { useState } from "react";
import { Network } from "lucide-react";

import { ObjectSection } from "@/components/portal/ObjectSection";
import { ObjectSummaryLink } from "@/components/portal/ObjectSummaryLink";
import { SurfaceState } from "@/components/portal/SurfaceState";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { useSimilarObjects } from "@/features/portal/queries";

const INITIAL_VISIBLE = 6;

export function SimilarDesigns({ iri }: { iri: string }) {
  const similar = useSimilarObjects(iri);
  const [expanded, setExpanded] = useState(false);

  return (
    <ObjectSection
      id="similar-designs"
      icon={Network}
      title="Similar designs"
      description="In-scope members of the same native sequence-similarity cluster, ordered by registry rank."
    >
      {similar.error ? (
        <SurfaceState
          variant="error"
          title="Similarity results are unavailable"
          description={(similar.error as Error).message}
        />
      ) : similar.isLoading || !similar.data ? (
        <div className="space-y-3" aria-label="Loading similar designs">
          <Skeleton className="h-36 rounded-xl" />
          <Skeleton className="h-36 rounded-xl" />
        </div>
      ) : similar.data.items.length === 0 ? (
        <SurfaceState
          title="No in-scope cluster neighbors"
          description="The object may have no indexed sequence or no sufficiently similar sequence visible to this account."
        />
      ) : (
        <>
          <div className="space-y-3">
            {similar.data.items
              .slice(0, expanded ? undefined : INITIAL_VISIBLE)
              .map((hit) => (
                <ObjectSummaryLink
                  key={hit.uri}
                  object={hit}
                  metadata={
                    <span className="text-[11px] tabular-nums text-muted-foreground">
                      Registry rank {hit.pagerank.toPrecision(3)}
                    </span>
                  }
                />
              ))}
          </div>
          {similar.data.total > INITIAL_VISIBLE && (
            <Button
              variant="outline"
              size="sm"
              className="mt-4"
              onClick={() => setExpanded((value) => !value)}
              aria-expanded={expanded}
            >
              {expanded
                ? "Show fewer"
                : `Show all ${similar.data.total.toLocaleString()}`}
            </Button>
          )}
        </>
      )}
    </ObjectSection>
  );
}
