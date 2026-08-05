import {
  ChevronDown,
  Command as CommandIcon,
  LogIn,
  Search,
  UserRound,
} from "lucide-react";
import type { ReactNode } from "react";
import { Link } from "react-router-dom";

import { ProductAccountMenu } from "@/components/product/ProductAccountMenu";
import { Button } from "@/components/ui/button";
import type { SessionUser } from "@/features/session/api";
import { cn } from "@/lib/utils";

export function ProductTopBar({
  children,
  signatureRail = true,
  className,
  contentClassName,
}: {
  children: ReactNode;
  signatureRail?: boolean;
  className?: string;
  contentClassName?: string;
}) {
  return (
    <header
      className={cn(
        "shrink-0 border-b border-foreground/15 bg-background/94 backdrop-blur-xl supports-[backdrop-filter]:bg-background/82",
        className
      )}
    >
      {signatureRail && <ProductSignatureRail />}
      <div
        className={cn(
          "flex h-[4.25rem] w-full items-center gap-3 px-4 sm:px-6 lg:px-8",
          contentClassName
        )}
      >
        {children}
      </div>
    </header>
  );
}

export function ProductTopBarActions({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn("ml-auto flex shrink-0 items-center gap-1.5", className)}
    >
      {children}
    </div>
  );
}

export function ProductCommandTrigger({
  onOpen,
  layout,
}: {
  onOpen: () => void;
  layout: "registry" | "admin";
}) {
  const registry = layout === "registry";
  if (registry) {
    return (
      <Button
        type="button"
        variant="outline"
        size="sm"
        aria-label="Open commands"
        title="Open commands (⌘K)"
        className="w-9 shrink-0 px-0 text-muted-foreground sm:w-auto sm:px-2.5"
        onClick={onOpen}
      >
        <CommandIcon />
        <kbd className="hidden font-mono text-[10px] text-muted-foreground/70 sm:inline">
          ⌘K
        </kbd>
      </Button>
    );
  }

  return (
    <Button
      type="button"
      variant="outline"
      size="sm"
      aria-label="Open command palette"
      className="w-9 shrink-0 justify-start px-0 text-muted-foreground lg:w-56 lg:px-3 xl:w-64"
      onClick={onOpen}
    >
      <Search />
      <span className="hidden lg:inline">Search commands…</span>
      <kbd className="ml-auto hidden font-mono text-[10px] text-muted-foreground/70 lg:inline">
        ⌘K
      </kbd>
    </Button>
  );
}

export function ProductAccountControl({
  user,
  surface,
  showSignedOut = false,
}: {
  user?: SessionUser | null;
  surface: "registry" | "admin";
  showSignedOut?: boolean;
}) {
  if (user) {
    return (
      <ProductAccountMenu user={user} surface={surface}>
        <Button
          variant="outline"
          size="sm"
          className="max-w-48 text-muted-foreground"
          aria-label={`Account menu for ${user.name}`}
        >
          <UserRound />
          <span
            className={cn(
              "truncate",
              surface === "registry" ? "hidden sm:inline" : "hidden xl:inline"
            )}
          >
            {user.name}
          </span>
          <ChevronDown className="size-3 text-muted-foreground" />
        </Button>
      </ProductAccountMenu>
    );
  }

  if (!showSignedOut) return null;
  return (
    <Button asChild variant="outline" size="sm">
      <Link to="/login">
        <LogIn />
        <span className="hidden sm:inline">Sign in</span>
      </Link>
    </Button>
  );
}

export function ProductSignatureRail({ className }: { className?: string }) {
  return (
    <div className={cn("flex h-1", className)} aria-hidden="true">
      <span className="flex-1 bg-sbol-promoter" />
      <span className="flex-[2] bg-sbol-rbs" />
      <span className="flex-1 bg-sbol-cds" />
      <span className="flex-1 bg-sbol-terminator" />
    </div>
  );
}
