import * as React from "react";

import { cn } from "@/lib/utils";

export type NativeSelectProps = React.ComponentPropsWithoutRef<"select">;

const NativeSelect = React.forwardRef<
  React.ElementRef<"select">,
  NativeSelectProps
>(({ className, children, ...props }, ref) => (
  <select
    ref={ref}
    className={cn(
      "flex h-9 w-full rounded-[3px] border border-input bg-background px-3 py-1 text-sm shadow-[inset_0_1px_0_hsl(var(--foreground)/0.03)] transition-[border-color,box-shadow] duration-150 focus-visible:outline-none focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/10 disabled:cursor-not-allowed disabled:opacity-50",
      className
    )}
    {...props}
  >
    {children}
  </select>
));
NativeSelect.displayName = "NativeSelect";

export { NativeSelect };
