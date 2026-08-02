import * as React from "react";
import { ChevronDown } from "lucide-react";

import { cn } from "@/lib/utils";

export type NativeSelectProps = React.ComponentPropsWithoutRef<"select">;

const NativeSelect = React.forwardRef<
  React.ElementRef<"select">,
  NativeSelectProps
>(({ className, children, ...props }, ref) => (
  <div className={cn("relative", className)}>
    <select
      ref={ref}
      className="peer flex h-9 w-full appearance-none rounded-[3px] border border-input bg-background py-1 pl-3 pr-9 text-sm shadow-[inset_0_1px_0_hsl(var(--foreground)/0.03)] transition-[border-color,box-shadow] duration-150 focus-visible:outline-none focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/10 disabled:cursor-not-allowed disabled:opacity-50"
      {...props}
    >
      {children}
    </select>
    <ChevronDown
      aria-hidden="true"
      className="pointer-events-none absolute right-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground peer-disabled:opacity-50"
    />
  </div>
));
NativeSelect.displayName = "NativeSelect";

export { NativeSelect };
