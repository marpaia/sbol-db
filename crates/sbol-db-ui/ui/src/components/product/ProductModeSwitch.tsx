import { Globe2, SlidersHorizontal } from "lucide-react";
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
    <div className={cn("space-y-1.5", className)}>
      <p className="px-1 font-mono text-[9px] uppercase tracking-[0.14em] text-sidebar-foreground/45">
        Workspace
      </p>
      <nav
        className="grid grid-cols-2 gap-0.5 rounded-[3px] border border-sidebar-border bg-black/10 p-0.5"
        aria-label="Workspace"
      >
        <ModeLink active={mode === "registry"} to="/" label="Registry">
          <Globe2 />
        </ModeLink>
        <ModeLink active={mode === "admin"} to="/admin" label="Admin">
          <SlidersHorizontal />
        </ModeLink>
      </nav>
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
        "group inline-flex h-8 items-center justify-center gap-2 rounded-[2px] px-2 font-mono text-[10px] font-medium uppercase tracking-[0.06em] outline-none ring-sidebar-ring transition-[color,background-color,box-shadow,transform] duration-150 [transition-timing-function:var(--ease-out)] focus-visible:ring-2 active:scale-[0.97] [&>svg]:size-3.5",
        active
          ? "bg-sidebar-accent text-sidebar-accent-foreground shadow-[inset_0_0_0_1px_hsl(var(--sidebar-border)),0_1px_2px_hsl(var(--sidebar-foreground)/0.06)] [&>svg]:text-sidebar-primary"
          : "text-sidebar-foreground/60 hover:bg-sidebar-accent/60 hover:text-sidebar-foreground"
      )}
    >
      {children}
      {label}
    </Link>
  );
}
