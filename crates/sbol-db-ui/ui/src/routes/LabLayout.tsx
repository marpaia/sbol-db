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

import { Fragment, useCallback } from "react";
import { Link, Outlet, useLocation, useNavigate } from "react-router-dom";

import { AppSidebar } from "@/components/lab/AppSidebar";
import { CommandPalette } from "@/components/lab/CommandPalette";
import { ProductThemeMenu } from "@/components/product/ProductThemeMenu";
import {
  ProductAccountControl,
  ProductCommandTrigger,
  ProductSignatureRail,
  ProductTopBar,
  ProductTopBarActions,
} from "@/components/product/ProductTopBar";
import { Separator } from "@/components/ui/separator";
import {
  SidebarInset,
  SidebarProvider,
  SidebarTrigger,
} from "@/components/ui/sidebar";
import { useSession } from "@/features/portal/queries";
import { useCommandPaletteShortcut } from "@/hooks/useCommandPaletteShortcut";
import { type Dialect, useLabStore } from "@/lib/store";
import { PRODUCT_NAME } from "@/lib/product";
import { adminPath } from "@/lib/routes";

export default function LabLayout() {
  const navigate = useNavigate();
  const { pathname } = useLocation();
  const session = useSession();
  const setBuffer = useLabStore((s) => s.setBuffer);

  // The command palette acts on whichever dialect the user is in. On
  // the dashboard (no active dialect), default to the last-used one.
  const lastDialect = useLabStore((s) => s.lastDialect);
  const activeDialect: Dialect = pathname.startsWith(adminPath("/sql"))
    ? "sql"
    : pathname.startsWith(adminPath("/sparql"))
      ? "sparql"
      : lastDialect;

  const loadQueryFor = useCallback(
    (targetDialect: Dialect, query: string) => {
      setBuffer(targetDialect, query);
      navigate(adminPath(`/${targetDialect}`));
    },
    [navigate, setBuffer]
  );

  const [paletteOpen, setPaletteOpen] = useCommandPaletteShortcut();

  return (
    <SidebarProvider className="admin-instrument h-svh">
      <ProductSignatureRail className="fixed inset-x-0 top-0 z-50" />
      <AppSidebar onOpenPalette={() => setPaletteOpen(true)} />
      <SidebarInset className="h-svh overflow-hidden">
        <ProductTopBar
          signatureRail={false}
          className="bg-card/75 pt-1 supports-[backdrop-filter]:bg-card/70"
          contentClassName="gap-2 px-3 sm:px-4 lg:px-5"
        >
          <SidebarTrigger className="-ml-1 h-9 w-9" />
          <Separator orientation="vertical" className="mx-1 h-4" />
          <Breadcrumb pathname={pathname} rootLabel={PRODUCT_NAME} />
          <ProductTopBarActions>
            <ProductCommandTrigger
              layout="admin"
              onOpen={() => setPaletteOpen(true)}
            />
            <ProductAccountControl user={session.data?.user} surface="admin" />
          </ProductTopBarActions>
        </ProductTopBar>
        <main className="flex-1 min-h-0 overflow-hidden">
          <Outlet />
        </main>
      </SidebarInset>
      <ProductThemeMenu floating />
      <CommandPalette
        open={paletteOpen}
        onOpenChange={setPaletteOpen}
        onLoadQuery={loadQueryFor}
        onSwitchDialect={(d) => navigate(adminPath(`/${d}`))}
      />
      {/* Suppress lint warning: activeDialect is used by the palette in
          future PRs (filter saved by dialect, etc). */}
      <span data-active-dialect={activeDialect} hidden />
    </SidebarProvider>
  );
}

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
  { prefix: adminPath("/import"), section: "Data model", page: "Import" },
  { prefix: adminPath("/graphs"), section: "Data model", page: "Graphs" },
  { prefix: adminPath("/objects"), section: "Data model", page: "Objects" },
  {
    prefix: adminPath("/sequences"),
    section: "Data model",
    page: "Sequences",
  },
  {
    prefix: adminPath("/ontologies"),
    section: "Data model",
    page: "Ontologies",
  },
  {
    prefix: adminPath("/neighborhood"),
    section: "Data model",
    page: "Neighborhood",
  },
  { prefix: adminPath("/schema"), section: "Query", page: "Schema" },
  { prefix: adminPath("/sparql"), section: "Query", page: "SPARQL" },
  { prefix: adminPath("/sql"), section: "Query", page: "SQL" },
  {
    prefix: adminPath("/settings/instance"),
    section: "Administration",
    page: "Instance",
  },
  {
    prefix: adminPath("/settings/users"),
    section: "Administration",
    page: "Users",
  },
  {
    prefix: adminPath("/settings/integrations"),
    section: "Administration",
    page: "Integrations",
  },
  {
    prefix: adminPath("/settings/edge"),
    section: "Administration",
    page: "Edge runtime",
  },
  {
    prefix: adminPath("/operations/search"),
    section: "Administration",
    page: "Search indexes",
  },
  {
    prefix: adminPath("/operations/backup"),
    section: "Administration",
    page: "Backups & recovery",
  },
  {
    prefix: adminPath("/operations/audit"),
    section: "Administration",
    page: "Activity",
  },
  {
    prefix: adminPath("/observability/maintenance"),
    section: "Operations",
    page: "Maintenance",
  },
  {
    prefix: adminPath("/observability/jobs"),
    section: "Operations",
    page: "Jobs",
  },
  {
    prefix: adminPath("/observability"),
    section: "Operations",
    page: "Metrics",
  },
];

function topLevelFor(
  pathname: string
): { section: string; page: string; root: string } | null {
  for (const entry of TOP_LEVEL_SECTIONS) {
    if (pathname === entry.prefix || pathname.startsWith(`${entry.prefix}/`)) {
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

  const ontologyMatch = pathname.match(/^\/admin\/ontologies\/([^/]+)\/?$/);
  if (ontologyMatch) {
    trail.push({
      label: decodeURIComponent(ontologyMatch[1]).toLowerCase(),
      mono: true,
    });
    return trail;
  }
  const tableMatch = pathname.match(/^\/admin\/schema\/tables\/([^/]+)\/?$/);
  if (tableMatch) {
    trail.push({ label: decodeURIComponent(tableMatch[1]), mono: true });
    return trail;
  }
  const graphMatch = pathname.match(/^\/admin\/graphs\/([^/]+)\/?$/);
  if (graphMatch) {
    trail.push({
      label: shortId(decodeURIComponent(graphMatch[1])),
      mono: true,
    });
    return trail;
  }
  const jobMatch = pathname.match(/^\/admin\/observability\/jobs\/([^/]+)\/?$/);
  if (jobMatch) {
    trail.push({
      label: shortId(decodeURIComponent(jobMatch[1])),
      mono: true,
    });
    return trail;
  }
  if (pathname === adminPath("/objects/lookup")) {
    trail.push({ label: "Bulk lookup" });
    return trail;
  }
  const objectMatch = pathname.match(/^\/admin\/objects\/([^/]+)\/?$/);
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

function Breadcrumb({
  pathname,
  rootLabel,
}: {
  pathname: string;
  rootLabel: string;
}) {
  const trail = buildTrail(pathname);
  return (
    <nav
      aria-label="Breadcrumb"
      className="flex min-w-0 items-center gap-2 overflow-hidden whitespace-nowrap text-sm"
    >
      <span className="text-muted-foreground">{rootLabel}</span>
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
