import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";

import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { cn } from "@/lib/utils";

export function ObjectSection({
  id,
  icon: Icon,
  title,
  description,
  action,
  children,
  className,
  contentClassName,
}: {
  id?: string;
  icon: LucideIcon;
  title: string;
  description?: string;
  action?: ReactNode;
  children: ReactNode;
  className?: string;
  contentClassName?: string;
}) {
  return (
    <Card
      id={id}
      className={cn(
        "scroll-mt-24 overflow-hidden border-foreground/15 bg-card/70",
        className
      )}
    >
      <CardHeader className="flex-row items-start justify-between gap-4 space-y-0 border-b border-foreground/15 bg-muted/10 p-5 sm:p-6">
        <div className="flex min-w-0 items-start gap-3.5">
          <span className="flex size-8 shrink-0 items-center justify-center border-l-2 border-primary text-primary">
            <Icon className="size-4" aria-hidden="true" />
          </span>
          <div className="min-w-0">
            <h2 className="text-xl font-medium tracking-tight">{title}</h2>
            {description && (
              <p className="mt-1 max-w-2xl text-sm leading-6 text-muted-foreground">
                {description}
              </p>
            )}
          </div>
        </div>
        {action}
      </CardHeader>
      <CardContent className={cn("p-5 sm:p-6", contentClassName)}>
        {children}
      </CardContent>
    </Card>
  );
}
