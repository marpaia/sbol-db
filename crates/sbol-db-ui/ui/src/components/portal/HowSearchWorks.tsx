import type { ReactNode } from "react";
import { ChevronDown } from "lucide-react";

import { cn } from "@/lib/utils";

export function HowSearchWorks({
  children,
  className,
  contentClassName,
}: {
  children: ReactNode;
  className?: string;
  contentClassName?: string;
}) {
  return (
    <details
      className={cn(
        "group mt-3 border-t pt-3 text-xs text-muted-foreground",
        className
      )}
    >
      <summary className="inline-flex cursor-pointer list-none items-center gap-1.5 rounded-md font-medium text-foreground outline-none transition-[color,transform] duration-150 [transition-timing-function:cubic-bezier(0.23,1,0.32,1)] hover:text-primary focus-visible:ring-2 focus-visible:ring-ring active:scale-[0.985] motion-reduce:transition-none [&::-webkit-details-marker]:hidden">
        How this search works
        <ChevronDown className="size-3.5 transition-transform duration-150 [transition-timing-function:cubic-bezier(0.23,1,0.32,1)] group-open:rotate-180 motion-reduce:transition-none" />
      </summary>
      <div
        className={cn(
          "mt-3 rounded-lg border bg-muted/10 p-3 leading-5",
          contentClassName
        )}
      >
        {children}
      </div>
    </details>
  );
}
