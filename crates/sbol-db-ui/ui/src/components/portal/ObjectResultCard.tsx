import { ArrowUpRight, Box } from "lucide-react";
import { Link } from "react-router-dom";

import { Badge } from "@/components/ui/badge";
import type { PortalSearchHit } from "@/features/portal/api";
import { shortIri } from "@/features/portal/format";
import { publicObjectPath } from "@/lib/routes";

export function ObjectResultCard({ hit }: { hit: PortalSearchHit }) {
  return (
    <Link
      to={publicObjectPath(hit.uri)}
      className="group flex min-h-44 flex-col rounded-xl border bg-card p-5 shadow-sm transition-[border-color,box-shadow] duration-150 [transition-timing-function:cubic-bezier(0.23,1,0.32,1)] hover:border-primary/35 hover:shadow-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
    >
      <div className="flex items-start gap-3">
        <span className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
          <Box className="size-4" />
        </span>
        <div className="min-w-0 flex-1">
          <h3 className="truncate font-semibold tracking-tight text-foreground group-hover:text-primary">
            {hit.name || hit.display_id || shortIri(hit.uri)}
          </h3>
          <div className="mt-1 truncate font-mono text-[11px] text-muted-foreground">
            {hit.display_id || shortIri(hit.uri)}
            {hit.version ? ` · v${hit.version}` : ""}
          </div>
        </div>
        <ArrowUpRight className="size-4 shrink-0 text-muted-foreground/50 group-hover:text-primary" />
      </div>

      <p className="mt-4 line-clamp-2 text-sm leading-6 text-muted-foreground">
        {hit.description || "No description has been provided for this object."}
      </p>

      <div className="mt-auto pt-4">
        <Badge variant="secondary" className="max-w-full font-mono text-[10px]">
          <span className="truncate">{shortIri(hit.object_type)}</span>
        </Badge>
      </div>
    </Link>
  );
}
