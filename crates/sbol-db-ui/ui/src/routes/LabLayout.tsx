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

import { Fragment, useCallback, useEffect, useState } from "react";
import { Command as CommandIcon } from "lucide-react";
import { Link, Outlet, useLocation, useNavigate } from "react-router-dom";

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

const ROOT_CRUMB = "SBOL Data Lab";

type Crumb = { label: string; to?: string; mono?: boolean };

/**
 * Each top-level route belongs to a sidebar section. The breadcrumb
 * prepends the section as a non-clickable crumb so the user can see
 * at a glance which group of features the current page lives in,
 * matching how the sidebar is organized.
 */
const TOP_LEVEL_SECTIONS: Array<{
  prefix: string;
  section: string;
  page: string;
}> = [
  { prefix: "/import", section: "Data", page: "Import" },
  { prefix: "/graphs", section: "Data", page: "Graphs" },
  { prefix: "/objects", section: "Data", page: "Objects" },
  { prefix: "/sequences", section: "Data", page: "Sequences" },
  { prefix: "/ontologies", section: "Data", page: "Ontologies" },
  { prefix: "/neighborhood", section: "Data", page: "Neighborhood" },
  { prefix: "/schema", section: "Query", page: "Schema" },
  { prefix: "/sparql", section: "Query", page: "SPARQL" },
  { prefix: "/sql", section: "Query", page: "SQL" },
  { prefix: "/observability/postgres", section: "Operations", page: "Postgres" },
  { prefix: "/observability/jobs", section: "Operations", page: "Jobs" },
  { prefix: "/observability", section: "Operations", page: "Metrics" },
];

function topLevelFor(
  pathname: string
): { section: string; page: string; root: string } | null {
  for (const entry of TOP_LEVEL_SECTIONS) {
    if (
      pathname === entry.prefix ||
      pathname.startsWith(`${entry.prefix}/`)
    ) {
      return { section: entry.section, page: entry.page, root: entry.prefix };
    }
  }
  return null;
}

function buildTrail(pathname: string): Crumb[] {
  const top = topLevelFor(pathname);
  if (!top) return [{ label: "Overview" }];

  const trail: Crumb[] = [
    { label: top.section },
    { label: top.page, to: top.root },
  ];

  const ontologyMatch = pathname.match(/^\/ontologies\/([^/]+)\/?$/);
  if (ontologyMatch) {
    trail.push({
      label: decodeURIComponent(ontologyMatch[1]).toLowerCase(),
      mono: true,
    });
    return trail;
  }
  const tableMatch = pathname.match(/^\/schema\/tables\/([^/]+)\/?$/);
  if (tableMatch) {
    trail.push({ label: decodeURIComponent(tableMatch[1]), mono: true });
    return trail;
  }
  const graphMatch = pathname.match(/^\/graphs\/([^/]+)\/?$/);
  if (graphMatch) {
    trail.push({
      label: shortId(decodeURIComponent(graphMatch[1])),
      mono: true,
    });
    return trail;
  }
  const jobMatch = pathname.match(/^\/observability\/jobs\/([^/]+)\/?$/);
  if (jobMatch) {
    trail.push({
      label: shortId(decodeURIComponent(jobMatch[1])),
      mono: true,
    });
    return trail;
  }
  if (pathname === "/objects/lookup") {
    trail.push({ label: "Bulk lookup" });
    return trail;
  }
  const objectMatch = pathname.match(/^\/objects\/([^/]+)\/?$/);
  if (objectMatch) {
    trail.push({
      label: shortLabel(decodeURIComponent(objectMatch[1])),
      mono: true,
    });
    return trail;
  }

  return trail;
}

function shortId(id: string): string {
  if (id.length <= 12) return id;
  return `${id.slice(0, 8)}…`;
}

function shortLabel(iri: string): string {
  const m = iri.match(/[#/]([^#/]+)$/);
  return m ? m[1] : iri.length > 32 ? `${iri.slice(0, 32)}…` : iri;
}

function Breadcrumb({ pathname }: { pathname: string }) {
  const trail = buildTrail(pathname);
  return (
    <nav aria-label="Breadcrumb" className="flex items-center gap-2 text-sm">
      <span className="text-muted-foreground">{ROOT_CRUMB}</span>
      {trail.map((crumb, i) => {
        const isLast = i === trail.length - 1;
        return (
          <Fragment key={i}>
            <span className="text-muted-foreground/40">/</span>
            {crumb.to && !isLast ? (
              <Link
                to={crumb.to}
                className="text-muted-foreground transition-colors hover:text-foreground"
              >
                {crumb.label}
              </Link>
            ) : !crumb.to && !isLast ? (
              <span className="text-muted-foreground">{crumb.label}</span>
            ) : (
              <span
                className={
                  crumb.mono
                    ? "font-mono font-medium text-foreground"
                    : "font-medium text-foreground"
                }
              >
                {crumb.label}
              </span>
            )}
          </Fragment>
        );
      })}
    </nav>
  );
}
