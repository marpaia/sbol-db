import {
  BookOpen,
  Dna,
  FilePlus2,
  FolderKanban,
  Globe2,
  Home,
  Info,
  LogIn,
  Search,
  SlidersHorizontal,
  UserPlus,
  UserRound,
} from "lucide-react";
import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";

import {
  ProductCommandPalette,
  ProductCommandPaletteGroup as PaletteGroup,
  ProductCommandPaletteItem as Item,
} from "@/components/product/ProductCommandPalette";
import { adminPath, API_DOCS_PATH } from "@/lib/routes";

export function RegistryCommandPalette({
  open,
  onOpenChange,
  authenticated,
  administrator,
  registrationOpen,
  setupRequired,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  authenticated: boolean;
  administrator: boolean;
  registrationOpen: boolean;
  setupRequired: boolean;
}) {
  const navigate = useNavigate();
  const [value, setValue] = useState("");

  useEffect(() => {
    if (!open) setValue("");
  }, [open]);

  const goTo = (path: string) => {
    navigate(path);
    onOpenChange(false);
  };

  const openReference = () => {
    window.open(API_DOCS_PATH, "_blank", "noopener,noreferrer");
    onOpenChange(false);
  };

  return (
    <ProductCommandPalette
      open={open}
      onOpenChange={onOpenChange}
      value={value}
      onValueChange={setValue}
      eyebrow="Registry commands"
      description="Navigate pages and run registry actions"
      placeholder="Jump to a page or action…"
      indexLabel="Command index"
      emptyDescription="Try workspace, contribute, search, or account."
    >
      <PaletteGroup heading="Discover" tone="promoter">
        <Item
          icon={<Home size={14} />}
          label="Registry home"
          onSelect={() => goTo("/")}
        />
        <Item
          icon={<Search size={14} />}
          label="Search biological designs"
          trailing="Search"
          onSelect={() => goTo("/search")}
        />
        <Item
          icon={<Dna size={14} />}
          label="Search by DNA sequence"
          trailing="Sequence"
          onSelect={() => goTo("/sequence-search")}
        />
      </PaletteGroup>

      <PaletteGroup heading="Understand" tone="terminator">
        <Item
          icon={<Info size={14} />}
          label="About SBOL DB"
          onSelect={() => goTo("/about")}
        />
      </PaletteGroup>

      {authenticated ? (
        <PaletteGroup heading="Your registry" tone="cds">
          <Item
            icon={<FilePlus2 size={14} />}
            label="Contribute designs"
            onSelect={() => goTo("/contribute")}
          />
          <Item
            icon={<FolderKanban size={14} />}
            label="Open workspace"
            onSelect={() => goTo("/workspace")}
          />
          <Item
            icon={<UserRound size={14} />}
            label="Account settings"
            onSelect={() => goTo("/account")}
          />
          {administrator && (
            <Item
              icon={<SlidersHorizontal size={14} />}
              label="Open admin workspace"
              trailing="Admin"
              onSelect={() => goTo(adminPath())}
            />
          )}
        </PaletteGroup>
      ) : (
        <PaletteGroup heading="Account" tone="cds">
          {setupRequired && (
            <Item
              icon={<SlidersHorizontal size={14} />}
              label="Set up this registry"
              trailing="Required"
              onSelect={() => goTo("/setup")}
            />
          )}
          <Item
            icon={<LogIn size={14} />}
            label="Sign in"
            onSelect={() => goTo("/login")}
          />
          {registrationOpen && (
            <Item
              icon={<UserPlus size={14} />}
              label="Create an account"
              onSelect={() => goTo("/register")}
            />
          )}
        </PaletteGroup>
      )}

      <PaletteGroup heading="Resources" tone="rbs">
        <Item
          icon={<BookOpen size={14} />}
          label="Open API reference"
          trailing="New tab"
          onSelect={openReference}
        />
        <Item
          icon={<Globe2 size={14} />}
          label="Learn about the SBOL standard"
          trailing="External"
          onSelect={() => {
            window.open(
              "https://sbolstandard.org",
              "_blank",
              "noopener,noreferrer"
            );
            onOpenChange(false);
          }}
        />
      </PaletteGroup>
    </ProductCommandPalette>
  );
}
