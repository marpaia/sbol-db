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
      "flex h-9 w-full rounded-md border border-input bg-background px-3 py-1 text-sm shadow-sm transition-[border-color,box-shadow] duration-150 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50",
      className
    )}
    {...props}
  >
    {children}
  </select>
));
NativeSelect.displayName = "NativeSelect";

export { NativeSelect };
