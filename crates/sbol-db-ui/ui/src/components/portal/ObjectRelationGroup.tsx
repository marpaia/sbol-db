import { useState } from "react";
import { ArrowUpRight, type LucideIcon } from "lucide-react";
import { Link } from "react-router-dom";

import { ObjectStateBadge } from "@/components/portal/ObjectStateBadge";
import { Button } from "@/components/ui/button";
import type { ObjectReferenceSection } from "@/features/portal/api";
import { shortIri } from "@/features/portal/format";
import { publicObjectPath } from "@/lib/routes";

const INITIAL_ITEMS = 5;

export function ObjectRelationGroup({
  icon: Icon,
  title,
  description,
  section,
  emptyLabel,
}: {
  icon: LucideIcon;
  title: string;
  description: string;
  section: ObjectReferenceSection;
  emptyLabel: string;
}) {
  const [expanded, setExpanded] = useState(false);
  const visibleItems = expanded
    ? section.items
    : section.items.slice(0, INITIAL_ITEMS);

  return (
    <section className="overflow-hidden rounded-xl border bg-card">
      <div className="flex items-start justify-between gap-3 border-b bg-muted/10 px-4 py-3.5">
        <div className="flex min-w-0 items-start gap-3">
          <span className="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary">
            <Icon className="size-3.5" aria-hidden="true" />
          </span>
          <div className="min-w-0">
            <h3 className="text-sm font-semibold">{title}</h3>
            <p className="mt-0.5 text-xs leading-5 text-muted-foreground">
              {description}
            </p>
          </div>
        </div>
        <ObjectStateBadge state={section.state} />
      </div>

      {section.items.length > 0 ? (
        <div className="p-2">
          <ul className="space-y-1" aria-label={title}>
            {visibleItems.map((item) => (
              <li key={item.uri}>
                <Link
                  to={publicObjectPath(item.uri)}
                  className="group flex min-h-11 items-center gap-3 rounded-lg px-2.5 py-2 text-sm transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  <span className="min-w-0 flex-1">
                    <span className="block truncate font-medium group-hover:text-primary">
                      {item.name || item.display_id || shortIri(item.uri)}
                    </span>
                    <span
                      className="mt-0.5 block truncate font-mono text-[10px] text-muted-foreground"
                      title={item.uri}
                    >
                      {item.uri}
                    </span>
                  </span>
                  <ArrowUpRight className="size-3.5 shrink-0 text-muted-foreground/50 group-hover:text-primary" />
                </Link>
              </li>
            ))}
          </ul>
          {section.items.length > INITIAL_ITEMS && (
            <Button
              variant="ghost"
              size="sm"
              className="mt-1 w-full"
              onClick={() => setExpanded((value) => !value)}
              aria-expanded={expanded}
            >
              {expanded
                ? "Show fewer"
                : `Show all ${section.items.length.toLocaleString()}`}
            </Button>
          )}
        </div>
      ) : (
        <div className="px-4 py-5 text-xs leading-5 text-muted-foreground">
          {section.note || emptyLabel}
        </div>
      )}

      {section.items.length > 0 && section.note && (
        <p className="border-t bg-amber-500/5 px-4 py-2.5 text-xs leading-5 text-amber-800 dark:text-amber-200">
          {section.note}
        </p>
      )}
    </section>
  );
}
