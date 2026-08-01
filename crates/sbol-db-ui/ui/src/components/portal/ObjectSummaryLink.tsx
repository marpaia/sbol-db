import { ArrowUpRight } from "lucide-react";
import { Link } from "react-router-dom";

import { ObjectTypeMark } from "@/components/portal/ObjectTypeMark";
import { Badge } from "@/components/ui/badge";
import type { PortalObjectSummary } from "@/features/portal/api";
import { shortIri } from "@/features/portal/format";
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
      className="group flex items-start gap-4 rounded-xl border bg-card p-4 shadow-sm transition-[border-color,box-shadow,transform] duration-150 [transition-timing-function:cubic-bezier(0.23,1,0.32,1)] hover:border-primary/35 hover:shadow-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 active:scale-[0.995] motion-reduce:transition-none sm:p-5"
    >
      <ObjectTypeMark
        objectType={object.object_type}
        className="size-10 rounded-lg"
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-start gap-3">
          <div className="min-w-0 flex-1">
            <h3 className="truncate font-semibold tracking-tight group-hover:text-primary">
              {object.name || object.display_id || shortIri(object.uri)}
            </h3>
            <p className="mt-1 truncate font-mono text-[11px] text-muted-foreground">
              {object.display_id || shortIri(object.uri)}
            </p>
          </div>
          <ArrowUpRight className="size-4 shrink-0 text-muted-foreground/50 group-hover:text-primary" />
        </div>
        {object.description && (
          <p className="mt-3 line-clamp-2 text-sm leading-6 text-muted-foreground">
            {object.description}
          </p>
        )}
        <div className="mt-4 flex flex-wrap items-center gap-2">
          <Badge variant="secondary" className="font-mono text-[10px]">
            {shortIri(object.object_type)}
          </Badge>
          {metadata}
        </div>
      </div>
    </Link>
  );
}
