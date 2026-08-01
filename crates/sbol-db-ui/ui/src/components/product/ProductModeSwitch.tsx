import { FlaskConical, Search } from "lucide-react";
import { Link } from "react-router-dom";

import { cn } from "@/lib/utils";

export function ProductModeSwitch({
  mode,
  className,
}: {
  mode: "registry" | "admin";
  className?: string;
}) {
  return (
    <div
      className={cn(
        "inline-flex items-center rounded-lg border bg-muted/45 p-0.5",
        className
      )}
      aria-label="Product area"
    >
      <ModeLink active={mode === "registry"} to="/" label="Registry">
        <Search />
      </ModeLink>
      <ModeLink active={mode === "admin"} to="/admin" label="Admin">
        <FlaskConical />
      </ModeLink>
    </div>
  );
}

function ModeLink({
  active,
  to,
  label,
  children,
}: {
  active: boolean;
  to: string;
  label: string;
  children: React.ReactNode;
}) {
  return (
    <Link
      to={to}
      aria-current={active ? "page" : undefined}
      className={cn(
        "inline-flex h-7 items-center gap-1.5 rounded-md px-2.5 text-xs font-medium transition-[color,background-color,box-shadow,transform] duration-150 [transition-timing-function:cubic-bezier(0.23,1,0.32,1)] active:scale-[0.97] [&>svg]:size-3.5",
        active
          ? "bg-background text-foreground shadow-sm"
          : "text-muted-foreground hover:text-foreground"
      )}
    >
      {children}
      {label}
    </Link>
  );
}
