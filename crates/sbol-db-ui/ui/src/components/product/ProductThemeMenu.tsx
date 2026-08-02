import { Monitor, Moon, Sun } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useTheme, type Theme } from "@/lib/theme";
import { cn } from "@/lib/utils";

export function ProductThemeMenu({ floating = false }: { floating?: boolean }) {
  const { theme, resolvedTheme, setTheme } = useTheme();
  const ActiveIcon = resolvedTheme === "dark" ? Moon : Sun;

  return (
    <div
      className={cn(
        floating &&
          "fixed bottom-4 right-4 z-40 sm:bottom-5 sm:right-5 lg:bottom-6 lg:right-6"
      )}
    >
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant={floating ? "outline" : "ghost"}
            size="icon"
            aria-label="Choose color theme"
            title="Appearance"
            className={cn(
              floating &&
                "size-10 rounded-full border-foreground/15 bg-background/90 shadow-lg backdrop-blur-xl hover:bg-accent"
            )}
          >
            <ActiveIcon />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          align="end"
          side={floating ? "top" : "bottom"}
          sideOffset={floating ? 8 : 4}
          className="w-40"
        >
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
    </div>
  );
}
