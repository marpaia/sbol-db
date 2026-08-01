import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  BookOpen,
  Cable,
  ChevronDown,
  Dna,
  FilePlus2,
  FolderKanban,
  LayoutDashboard,
  LogIn,
  LogOut,
  Menu,
  Monitor,
  Moon,
  Search,
  Settings,
  Sun,
  UserRound,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { Link, NavLink, Outlet, useNavigate } from "react-router-dom";

import { BrandMark } from "@/components/lab/BrandMark";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { deleteSession } from "@/features/portal/api";
import { portalKeys, useInstance, useSession } from "@/features/portal/queries";
import { deploymentName, PRODUCT_NAME, PRODUCT_TAGLINE } from "@/lib/product";
import { useTheme, type Theme } from "@/lib/theme";
import { cn } from "@/lib/utils";

type NavItem = {
  to: string;
  label: string;
  icon: LucideIcon;
  external?: boolean;
};

const navItems: NavItem[] = [
  { to: "/search", label: "Browse", icon: Search },
  { to: "/sequence-search", label: "Sequence search", icon: Dna },
  { to: "/connect", label: "Connect", icon: Cable },
  { to: "/api/v2/docs", label: "API", icon: BookOpen, external: true },
];

const accountNavItems: NavItem[] = [
  { to: "/contribute", label: "Contribute", icon: FilePlus2 },
  { to: "/workspace", label: "Workspace", icon: FolderKanban },
];

export default function PublicShell() {
  const instance = useInstance();
  const session = useSession();
  const deployment = deploymentName(instance.data?.name);
  const visibleNavItems = session.data?.authenticated
    ? [...navItems.slice(0, 2), ...accountNavItems, ...navItems.slice(2)]
    : navItems;

  return (
    <div className="flex min-h-svh flex-col bg-background">
      <header className="sticky top-0 z-40 border-b bg-background/90 backdrop-blur-xl supports-[backdrop-filter]:bg-background/75">
        <div className="mx-auto flex h-16 w-full max-w-7xl items-center gap-6 px-4 sm:px-6 lg:px-8">
          <Link
            to="/"
            className="flex min-w-0 items-center gap-3 rounded-lg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
          >
            <BrandMark className="size-9 rounded-xl" />
            <div className="min-w-0">
              <div className="truncate text-sm font-semibold tracking-tight">
                {PRODUCT_NAME}
              </div>
              <div className="hidden text-[11px] text-muted-foreground sm:block">
                {deployment || PRODUCT_TAGLINE}
              </div>
            </div>
          </Link>

          <nav
            aria-label="Primary navigation"
            className="hidden items-center gap-1 md:flex"
          >
            {visibleNavItems.map((item) =>
              item.external ? (
                <a
                  key={item.to}
                  href={item.to}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="inline-flex h-9 items-center gap-2 rounded-md px-3 text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                >
                  <item.icon className="size-3.5" />
                  {item.label}
                </a>
              ) : (
                <NavLink
                  key={item.to}
                  to={item.to}
                  className={({ isActive }) =>
                    cn(
                      "inline-flex h-9 items-center gap-2 rounded-md px-3 text-sm transition-colors",
                      isActive
                        ? "bg-accent text-accent-foreground"
                        : "text-muted-foreground hover:bg-accent hover:text-foreground"
                    )
                  }
                >
                  <item.icon className="size-3.5" />
                  {item.label}
                </NavLink>
              )
            )}
          </nav>

          <div className="ml-auto flex items-center gap-1.5">
            <MobileNavigation
              authenticated={Boolean(session.data?.authenticated)}
            />
            <ThemeMenu />
            {session.data?.authenticated && session.data.user ? (
              <AccountMenu
                name={session.data.user.name}
                isAdmin={session.data.user.is_admin}
              />
            ) : (
              <Button asChild variant="outline" size="sm">
                <Link to="/login">
                  <LogIn />
                  <span className="hidden sm:inline">Sign in</span>
                </Link>
              </Button>
            )}
          </div>
        </div>
      </header>

      {instance.data?.setup_required && (
        <div className="border-b border-primary/20 bg-primary/5">
          <div className="mx-auto flex max-w-7xl items-center justify-between gap-4 px-4 py-2.5 text-sm sm:px-6 lg:px-8">
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

      <footer className="border-t bg-muted/20">
        <div className="mx-auto flex max-w-7xl flex-col gap-3 px-4 py-8 text-xs text-muted-foreground sm:flex-row sm:items-center sm:justify-between sm:px-6 lg:px-8">
          <div className="flex items-center gap-2">
            <BrandMark className="size-6 rounded-md" />
            <span>
              {PRODUCT_NAME}
              {deployment ? ` · ${deployment}` : ` · ${PRODUCT_TAGLINE}`}
            </span>
          </div>
          <div className="flex items-center gap-4">
            <Link className="hover:text-foreground" to="/connect">
              Connect tools
            </Link>
            <a className="hover:text-foreground" href="/api/v2/docs">
              API reference
            </a>
            <a
              className="hover:text-foreground"
              href="https://sbolstandard.org"
              target="_blank"
              rel="noopener noreferrer"
            >
              About SBOL
            </a>
          </div>
        </div>
      </footer>
    </div>
  );
}

function MobileNavigation({ authenticated }: { authenticated: boolean }) {
  const visibleNavItems = authenticated
    ? [...navItems.slice(0, 2), ...accountNavItems, ...navItems.slice(2)]
    : navItems;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          className="md:hidden"
          aria-label="Open navigation"
        >
          <Menu />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-56 md:hidden">
        <DropdownMenuLabel>Explore SBOL DB</DropdownMenuLabel>
        <DropdownMenuSeparator />
        {visibleNavItems.map((item) =>
          item.external ? (
            <DropdownMenuItem key={item.to} asChild>
              <a href={item.to} target="_blank" rel="noopener noreferrer">
                <item.icon />
                {item.label}
              </a>
            </DropdownMenuItem>
          ) : (
            <DropdownMenuItem key={item.to} asChild>
              <Link to={item.to}>
                <item.icon />
                {item.label}
              </Link>
            </DropdownMenuItem>
          )
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function AccountMenu({ name, isAdmin }: { name: string; isAdmin: boolean }) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const logout = useMutation({
    mutationFn: deleteSession,
    onSuccess: () => {
      queryClient.setQueryData(portalKeys.session, {
        authenticated: false,
        user: null,
      });
      navigate("/");
    },
  });

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="outline"
          size="sm"
          className="max-w-48"
          aria-label={`Account menu for ${name}`}
        >
          <UserRound />
          <span className="hidden truncate sm:inline">{name}</span>
          <ChevronDown className="size-3 text-muted-foreground" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-56">
        <DropdownMenuLabel className="truncate">{name}</DropdownMenuLabel>
        <DropdownMenuSeparator />
        {isAdmin && (
          <DropdownMenuItem asChild>
            <Link to="/admin">
              <LayoutDashboard />
              Admin workspace
            </Link>
          </DropdownMenuItem>
        )}
        <DropdownMenuItem asChild>
          <Link to="/account">
            <Settings />
            Account settings
          </Link>
        </DropdownMenuItem>
        <DropdownMenuItem
          disabled={logout.isPending}
          onSelect={() => logout.mutate()}
        >
          <LogOut />
          {logout.isPending ? "Signing out…" : "Sign out"}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function ThemeMenu() {
  const { theme, resolvedTheme, setTheme } = useTheme();
  const ActiveIcon = resolvedTheme === "dark" ? Moon : Sun;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon" aria-label="Choose color theme">
          <ActiveIcon />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-40">
        <DropdownMenuRadioGroup
          value={theme}
          onValueChange={(value) => setTheme(value as Theme)}
        >
          <DropdownMenuRadioItem value="light">
            <Sun className="mr-2 size-4" /> Light
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="dark">
            <Moon className="mr-2 size-4" /> Dark
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="system">
            <Monitor className="mr-2 size-4" /> System
          </DropdownMenuRadioItem>
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
