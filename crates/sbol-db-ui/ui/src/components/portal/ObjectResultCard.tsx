import { ArrowUpRight, CalendarDays, Dna, UserRound } from "lucide-react";
import { Link } from "react-router-dom";

import { ObjectTypeMark } from "@/components/portal/ObjectTypeMark";
import { Badge } from "@/components/ui/badge";
import type { PortalSearchHit } from "@/features/portal/api";
import { shortIri } from "@/features/portal/format";
import { publicObjectPath } from "@/lib/routes";
import { cn } from "@/lib/utils";

export function ObjectResultCard({
  hit,
  variant = "card",
}: {
  hit: PortalSearchHit;
  variant?: "card" | "row";
}) {
  const date = hit.modified_at || hit.created_at;
  return (
    <Link
      to={publicObjectPath(hit.uri)}
      className={cn(
        "group relative flex bg-card transition-[background-color,border-color,transform] duration-150 [transition-timing-function:var(--ease-out)] hover:bg-accent/25 focus-visible:z-10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset active:scale-[0.998] motion-reduce:transition-none",
        variant === "card"
          ? "min-h-56 flex-col border border-foreground/15 p-5 before:absolute before:inset-x-0 before:top-0 before:h-0.5 before:bg-primary/70"
          : "items-start gap-4 bg-transparent px-2 py-5 sm:px-3 sm:py-6"
      )}
    >
      <ObjectTypeMark
        objectType={hit.object_type}
        className={cn(variant === "card" ? "size-9" : "size-10")}
      />

      <div className="flex min-w-0 flex-1 flex-col self-stretch">
        <div className="flex min-w-0 items-start gap-3">
          <div className="min-w-0 flex-1">
            <h3 className="record-title truncate text-lg font-medium tracking-tight text-foreground group-hover:text-primary">
              {hit.name || hit.display_id || shortIri(hit.uri)}
            </h3>
            <div className="mt-1 truncate font-mono text-[11px] text-muted-foreground">
              {hit.display_id || shortIri(hit.uri)}
              {hit.version ? ` · v${hit.version}` : ""}
            </div>
          </div>
          <ArrowUpRight className="size-4 shrink-0 text-muted-foreground/50 transition-transform duration-150 [transition-timing-function:var(--ease-out)] group-hover:-translate-y-0.5 group-hover:translate-x-0.5 group-hover:text-primary motion-reduce:transition-none" />
        </div>

        <p
          className={cn(
            "mt-3 text-sm leading-6 text-muted-foreground",
            variant === "card" ? "line-clamp-2" : "line-clamp-2 max-w-3xl"
          )}
        >
          {hit.description ||
            "No description has been provided for this object."}
        </p>

        <div className="mt-auto flex flex-wrap items-center gap-x-3 gap-y-2 pt-4 text-[11px] text-muted-foreground">
          <Badge
            variant="secondary"
            className="max-w-full rounded-[2px] border border-foreground/10 bg-transparent font-mono text-[10px]"
          >
            <span className="truncate">{shortIri(hit.object_type)}</span>
          </Badge>
          {hit.roles[0] && (
            <span className="inline-flex min-w-0 items-center gap-1">
              <Dna className="size-3 text-primary" />
              <span className="max-w-40 truncate">
                {shortIri(hit.roles[0])}
              </span>
              {hit.roles.length > 1 && ` +${hit.roles.length - 1}`}
            </span>
          )}
          {hit.owners[0] && (
            <span className="inline-flex min-w-0 items-center gap-1">
              <UserRound className="size-3" />
              <span className="max-w-32 truncate">
                {shortIri(hit.owners[0])}
              </span>
            </span>
          )}
          {date && (
            <span className="inline-flex items-center gap-1">
              <CalendarDays className="size-3" />
              <time dateTime={date}>{formatDate(date)}</time>
            </span>
          )}
        </div>
      </div>
    </Link>
  );
}

function formatDate(value: string): string {
  const day = value.slice(0, 10);
  const [year, month, date] = day.split("-").map(Number);
  if (!year || !month || !date) return day;
  return new Intl.DateTimeFormat(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    timeZone: "UTC",
  }).format(new Date(Date.UTC(year, month - 1, date)));
}
