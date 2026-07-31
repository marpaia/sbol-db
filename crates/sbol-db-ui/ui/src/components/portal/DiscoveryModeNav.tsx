import { Braces, Dna } from "lucide-react";
import { NavLink } from "react-router-dom";

import { cn } from "@/lib/utils";

const modes = [
  { to: "/search", label: "Metadata & facets", icon: Braces },
  { to: "/sequence-search", label: "DNA sequence", icon: Dna },
];

export function DiscoveryModeNav() {
  return (
    <nav
      aria-label="Discovery mode"
      className="mt-5 inline-flex rounded-lg border bg-muted/20 p-1"
    >
      {modes.map((mode) => (
        <NavLink
          key={mode.to}
          to={mode.to}
          end
          className={({ isActive }) =>
            cn(
              "inline-flex h-8 items-center gap-2 rounded-md px-3 text-xs font-medium transition-[color,background-color,box-shadow] duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 motion-reduce:transition-none",
              isActive
                ? "bg-background text-foreground shadow-sm"
                : "text-muted-foreground hover:text-foreground"
            )
          }
        >
          <mode.icon className="size-3.5" />
          {mode.label}
        </NavLink>
      ))}
    </nav>
  );
}
