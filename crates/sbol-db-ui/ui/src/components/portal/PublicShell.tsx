import {
  BookOpen,
  ChevronDown,
  FilePlus2,
  FolderKanban,
  Info,
  Menu,
  Search,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { Link, Outlet, useLocation } from "react-router-dom";

import { BrandMark } from "@/components/lab/BrandMark";
import { RegistryCommandPalette } from "@/components/portal/RegistryCommandPalette";
import { ProductThemeMenu } from "@/components/product/ProductThemeMenu";
import {
  ProductAccountControl,
  ProductCommandTrigger,
  ProductTopBar,
  ProductTopBarActions,
} from "@/components/product/ProductTopBar";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useInstance, useSession } from "@/features/portal/queries";
import { useCommandPaletteShortcut } from "@/hooks/useCommandPaletteShortcut";
import { deploymentName, PRODUCT_NAME, PRODUCT_TAGLINE } from "@/lib/product";
import { cn } from "@/lib/utils";

type NavItem = {
  to: string;
  label: string;
  icon: LucideIcon;
  external?: boolean;
};

const workNavItems: NavItem[] = [
  { to: "/workspace", label: "Workspace", icon: FolderKanban },
  { to: "/contribute", label: "Contribute", icon: FilePlus2 },
];

const resourceNavItems: NavItem[] = [
  { to: "/about", label: "About", icon: Info },
  {
    to: "/api/v2/docs",
    label: "API reference",
    icon: BookOpen,
    external: true,
  },
];

export default function PublicShell() {
  const { pathname } = useLocation();
  const instance = useInstance();
  const session = useSession();
  const [paletteOpen, setPaletteOpen] = useCommandPaletteShortcut();
  const deployment = deploymentName(instance.data?.name);
  const authenticated = Boolean(session.data?.authenticated);
  const searchActive = routeMatches(pathname, "/search");

  return (
    <div className="public-registry flex min-h-svh flex-col bg-background">
      <ProductTopBar
        className="sticky top-0 z-40"
        contentClassName="gap-3 px-3 sm:px-4 lg:px-5"
      >
        <Link
          to="/"
          className="flex min-w-0 shrink-0 items-center gap-3 rounded-[4px] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
        >
          <BrandMark className="size-10" />
          <div className="min-w-0">
            <div className="truncate font-mono text-sm font-semibold tracking-[0.08em]">
              {PRODUCT_NAME}
            </div>
            <div className="hidden text-[10px] uppercase tracking-[0.12em] text-muted-foreground sm:block">
              {deployment || PRODUCT_TAGLINE}
            </div>
          </div>
        </Link>

        <nav
          aria-label="Primary navigation"
          className="hidden h-full items-stretch border-l border-foreground/10 xl:flex"
        >
          {authenticated && (
            <DesktopNavMenu
              label="Your work"
              items={workNavItems}
              pathname={pathname}
            />
          )}
          <DesktopNavMenu
            label="Resources"
            items={resourceNavItems}
            pathname={pathname}
          />
        </nav>

        {!searchActive && <RegistrySearchTrigger />}

        <ProductTopBarActions className={searchActive ? undefined : "ml-0"}>
          {!searchActive && (
            <Button asChild variant="ghost" size="icon" className="lg:hidden">
              <Link to="/search" aria-label="Search registry">
                <Search />
              </Link>
            </Button>
          )}
          <ProductCommandTrigger
            layout="registry"
            onOpen={() => setPaletteOpen(true)}
          />
          <MobileNavigation authenticated={authenticated} />
          <ProductAccountControl
            user={session.data?.user}
            surface="registry"
            showSignedOut
          />
        </ProductTopBarActions>
      </ProductTopBar>

      <ProductThemeMenu floating />

      {instance.data?.setup_required && (
        <div className="border-b border-primary/20 bg-primary/5">
          <div className="mx-auto flex max-w-[90rem] items-center justify-between gap-4 px-4 py-2.5 text-sm sm:px-6 lg:px-8">
            <span>This registry needs its first administrator.</span>
            <Button asChild size="sm">
              <Link to="/setup">Set up instance</Link>
            </Button>
          </div>
        </div>
      )}

      <main className="flex-1">
        <Outlet />
      </main>

      <footer className="border-t border-foreground/15 bg-card text-card-foreground">
        <div className="mx-auto grid max-w-[90rem] gap-8 px-4 py-10 text-xs text-muted-foreground sm:grid-cols-[1.4fr_0.6fr] sm:px-6 lg:px-8">
          <div className="flex max-w-xl items-start gap-3">
            <BrandMark className="size-8" />
            <div>
              <div className="font-mono text-sm font-semibold tracking-[0.08em] text-foreground">
                {PRODUCT_NAME}
              </div>
              <p className="mt-1 leading-5">{deployment || PRODUCT_TAGLINE}.</p>
            </div>
          </div>
          <nav
            aria-label="Footer navigation"
            className="flex flex-wrap content-start gap-x-5 gap-y-3 sm:justify-end"
          >
            <span className="flex gap-5">
              <a
                className="text-muted-foreground hover:text-foreground"
                href="/api/v2/docs"
              >
                API reference
              </a>
            </span>
            <span className="flex gap-5">
              <Link
                className="text-muted-foreground hover:text-foreground"
                to="/about"
              >
                About SBOL DB
              </Link>
              <a
                className="text-muted-foreground hover:text-foreground"
                href="https://sbolstandard.org"
                target="_blank"
                rel="noopener noreferrer"
              >
                About SBOL
              </a>
            </span>
            <span className="flex gap-5">
              <Link
                className="text-muted-foreground hover:text-foreground"
                to="/privacy"
              >
                Privacy
              </Link>
              <Link
                className="text-muted-foreground hover:text-foreground"
                to="/terms"
              >
                Terms
              </Link>
            </span>
          </nav>
        </div>
      </footer>

      <RegistryCommandPalette
        open={paletteOpen}
        onOpenChange={setPaletteOpen}
        authenticated={authenticated}
        administrator={Boolean(session.data?.user?.is_admin)}
        registrationOpen={Boolean(
          instance.data?.setup_required ||
          instance.data?.policies.allow_public_signup
        )}
        setupRequired={Boolean(instance.data?.setup_required)}
      />
    </div>
  );
}

function MobileNavigation({ authenticated }: { authenticated: boolean }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          className="xl:hidden"
          aria-label="Open navigation"
        >
          <Menu />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-56 xl:hidden">
        {authenticated && (
          <>
            <DropdownMenuLabel>Your work</DropdownMenuLabel>
            {workNavItems.map((item) => (
              <NavMenuItem key={item.to} item={item} />
            ))}
          </>
        )}
        {authenticated && <DropdownMenuSeparator />}
        <DropdownMenuLabel>Resources</DropdownMenuLabel>
        {resourceNavItems.map((item) => (
          <NavMenuItem key={item.to} item={item} />
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function RegistrySearchTrigger() {
  return (
    <div className="hidden min-w-64 flex-1 justify-center lg:flex">
      <Button
        asChild
        variant="secondary"
        className="h-10 w-full max-w-xl justify-start bg-secondary/70 px-4 shadow-none hover:bg-secondary"
      >
        <Link to="/search" aria-label="Open registry search">
          <Search aria-hidden="true" />
          <span>Search registry</span>
        </Link>
      </Button>
    </div>
  );
}

function DesktopNavMenu({
  label,
  items,
  pathname,
}: {
  label: string;
  items: NavItem[];
  pathname: string;
}) {
  const active = items.some(
    (item) => !item.external && routeMatches(pathname, item.to)
  );
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button type="button" className={navTriggerClass(active)}>
          {label}
          <ChevronDown className="size-3.5 text-muted-foreground" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-52">
        {items.map((item) => (
          <NavMenuItem key={item.to} item={item} />
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function NavMenuItem({ item }: { item: NavItem }) {
  return (
    <DropdownMenuItem asChild>
      {item.external ? (
        <a href={item.to} target="_blank" rel="noopener noreferrer">
          <item.icon />
          {item.label}
        </a>
      ) : (
        <Link to={item.to}>
          <item.icon />
          {item.label}
        </Link>
      )}
    </DropdownMenuItem>
  );
}

function navTriggerClass(active: boolean): string {
  return cn(
    "relative inline-flex h-full items-center gap-2 border-r border-foreground/10 px-4 text-sm outline-none transition-colors duration-150 after:absolute after:inset-x-4 after:bottom-0 after:h-0.5 after:origin-left after:scale-x-0 after:bg-primary after:transition-transform after:duration-150 after:[transition-timing-function:var(--ease-out)] focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring active:scale-[0.98]",
    active
      ? "bg-accent/55 text-accent-foreground after:scale-x-100"
      : "text-muted-foreground hover:bg-accent/40 hover:text-foreground"
  );
}

function routeMatches(pathname: string, to: string): boolean {
  return pathname === to || pathname.startsWith(`${to}/`);
}
