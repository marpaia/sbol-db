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

import { adminBreadcrumbs, type AdminCrumb } from "@/app/routing/adminManifest";
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
import { useSession } from "@/features/session/queries";
import { useCommandPaletteShortcut } from "@/shared/hooks/useCommandPaletteShortcut";
import {
  type Dialect,
  useWorkbenchStore,
} from "@/features/admin/workbench/store";
import { PRODUCT_NAME } from "@/lib/product";
import { adminPath } from "@/lib/routes";

export default function LabLayout() {
  const navigate = useNavigate();
  const { pathname } = useLocation();
  const session = useSession();
  const setBuffer = useWorkbenchStore((s) => s.setBuffer);

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
    </SidebarProvider>
  );
}

function Breadcrumb({
  pathname,
  rootLabel,
}: {
  pathname: string;
  rootLabel: string;
}) {
  const trail: AdminCrumb[] = adminBreadcrumbs(pathname);
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
