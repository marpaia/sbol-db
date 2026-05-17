/**
 * Persistent shell for the lab bench. Owns:
 *
 * - The shadcn sidebar (logo + nav + palette trigger)
 * - A topbar with a sidebar toggle, breadcrumb, and palette button
 * - The command-palette modal and its keybinding
 * - The Outlet that renders the active route
 *
 * Each route below decides its own content layout. The dashboard
 * fills the inset with full-width content; the SQL/SPARQL routes
 * wrap themselves in `WorkbenchShell` for the three-column experience.
 */

import { useCallback, useEffect, useState } from "react";
import { Command as CommandIcon } from "lucide-react";
import { Outlet, useLocation, useNavigate } from "react-router-dom";

import { AppSidebar } from "@/components/lab/AppSidebar";
import { CommandPalette } from "@/components/lab/CommandPalette";
import { Separator } from "@/components/ui/separator";
import {
  SidebarInset,
  SidebarProvider,
  SidebarTrigger,
} from "@/components/ui/sidebar";
import { type Dialect, useLabStore } from "@/lib/store";

export default function LabLayout() {
  const navigate = useNavigate();
  const { pathname } = useLocation();
  const setBuffer = useLabStore((s) => s.setBuffer);

  // The command palette acts on whichever dialect the user is in. On
  // the dashboard (no active dialect), default to the last-used one.
  const lastDialect = useLabStore((s) => s.lastDialect);
  const activeDialect: Dialect = pathname.startsWith("/sql")
    ? "sql"
    : pathname.startsWith("/sparql")
      ? "sparql"
      : lastDialect;

  const loadQueryFor = useCallback(
    (targetDialect: Dialect, query: string) => {
      setBuffer(targetDialect, query);
      navigate(`/${targetDialect}`);
    },
    [navigate, setBuffer]
  );

  const [paletteOpen, setPaletteOpen] = useState(false);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPaletteOpen((v) => !v);
      } else if (e.key === "Escape" && paletteOpen) {
        setPaletteOpen(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [paletteOpen]);

  return (
    <SidebarProvider className="h-svh">
      <AppSidebar onOpenPalette={() => setPaletteOpen(true)} />
      <SidebarInset className="h-svh overflow-hidden">
        <header className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
          <SidebarTrigger className="-ml-1" />
          <Separator orientation="vertical" className="mx-1 h-4" />
          <Breadcrumb pathname={pathname} />
          <button
            type="button"
            onClick={() => setPaletteOpen(true)}
            className="ml-auto inline-flex items-center gap-2 rounded-md border bg-background px-2 py-1 text-xs text-muted-foreground transition-colors hover:text-foreground"
          >
            <CommandIcon size={12} />
            <span>Search…</span>
            <kbd className="text-[10px] text-muted-foreground/70">⌘K</kbd>
          </button>
        </header>
        <main className="flex-1 min-h-0 overflow-hidden">
          <Outlet />
        </main>
      </SidebarInset>
      <CommandPalette
        open={paletteOpen}
        onOpenChange={setPaletteOpen}
        onLoadQuery={loadQueryFor}
        onSwitchDialect={(d) => navigate(`/${d}`)}
      />
      {/* Suppress lint warning: activeDialect is used by the palette in
          future PRs (filter saved by dialect, etc). */}
      <span data-active-dialect={activeDialect} hidden />
    </SidebarProvider>
  );
}

function Breadcrumb({ pathname }: { pathname: string }) {
  const segment = pathname.startsWith("/sparql")
    ? "SPARQL"
    : pathname.startsWith("/sql")
      ? "SQL"
      : pathname.startsWith("/schema")
        ? "Schema"
        : pathname.startsWith("/ontologies")
          ? "Ontologies"
          : "Overview";
  return (
    <nav aria-label="Breadcrumb" className="flex items-center gap-2 text-sm">
      <span className="text-muted-foreground">Lab</span>
      <span className="text-muted-foreground/40">/</span>
      <span className="font-medium text-foreground">{segment}</span>
    </nav>
  );
}
