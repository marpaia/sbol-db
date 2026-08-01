import { useMutation, useQueryClient } from "@tanstack/react-query";
import { FlaskConical, LogOut, Search, Settings } from "lucide-react";
import type { ReactNode } from "react";
import { Link, useNavigate } from "react-router-dom";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { deleteSession, type SessionUser } from "@/features/portal/api";
import { portalKeys } from "@/features/portal/queries";

export function ProductAccountMenu({
  user,
  surface,
  align = "end",
  children,
}: {
  user: SessionUser;
  surface: "registry" | "admin";
  align?: "start" | "center" | "end";
  children: ReactNode;
}) {
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
      <DropdownMenuTrigger asChild>{children}</DropdownMenuTrigger>
      <DropdownMenuContent align={align} className="w-64">
        <DropdownMenuLabel>
          <span className="block truncate">{user.name}</span>
          <span className="mt-0.5 block truncate text-[11px] font-normal text-muted-foreground">
            {user.email || `@${user.username}`}
          </span>
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        {surface === "registry" && user.is_admin && (
          <DropdownMenuItem asChild>
            <Link to="/admin">
              <FlaskConical />
              Admin workspace
            </Link>
          </DropdownMenuItem>
        )}
        {surface === "admin" && (
          <DropdownMenuItem asChild>
            <Link to="/">
              <Search />
              Registry home
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
