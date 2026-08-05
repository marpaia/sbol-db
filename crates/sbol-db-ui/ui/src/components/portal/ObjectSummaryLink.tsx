import { ArrowUpRight } from "lucide-react";
import { Link } from "react-router-dom";

import { ObjectTypeMark } from "@/components/portal/ObjectTypeMark";
import { Badge } from "@/components/ui/badge";
import type { PortalObjectSummary } from "@/features/registry/objects/api";
import { shortIri } from "@/features/registry/objects/format";
import { publicObjectPath } from "@/lib/routes";

export function ObjectSummaryLink({
  object,
  metadata,
}: {
  object: PortalObjectSummary;
  metadata?: React.ReactNode;
}) {
  return (
    <Link
      to={publicObjectPath(object.uri)}
      className="group flex items-start gap-4 bg-transparent px-2 py-5 transition-[background-color,transform] duration-150 [transition-timing-function:var(--ease-out)] hover:bg-accent/25 focus-visible:z-10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring active:scale-[0.998] motion-reduce:transition-none sm:px-3 sm:py-6"
    >
      <ObjectTypeMark objectType={object.object_type} className="size-10" />
      <div className="min-w-0 flex-1">
        <div className="flex items-start gap-3">
          <div className="min-w-0 flex-1">
            <h3 className="record-title truncate text-lg font-medium tracking-tight group-hover:text-primary">
              {object.name || object.display_id || shortIri(object.uri)}
            </h3>
            <p className="mt-1 truncate font-mono text-[11px] text-muted-foreground">
              {object.display_id || shortIri(object.uri)}
            </p>
          </div>
          <ArrowUpRight className="size-4 shrink-0 text-muted-foreground/50 transition-transform duration-150 [transition-timing-function:var(--ease-out)] group-hover:-translate-y-0.5 group-hover:translate-x-0.5 group-hover:text-primary motion-reduce:transition-none" />
        </div>
        {object.description && (
          <p className="mt-3 line-clamp-2 text-sm leading-6 text-muted-foreground">
            {object.description}
          </p>
        )}
        <div className="mt-4 flex flex-wrap items-center gap-2">
          <Badge
            variant="secondary"
            className="rounded-[2px] border border-foreground/10 bg-transparent font-mono text-[10px]"
          >
            {shortIri(object.object_type)}
          </Badge>
          {metadata}
        </div>
      </div>
    </Link>
  );
}
