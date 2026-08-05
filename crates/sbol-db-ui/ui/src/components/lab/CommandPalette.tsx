/**
 * ⌘K command palette. Backed by `cmdk` — fuzzy search across:
 *
 * - Saved queries (load into the appropriate dialect)
 * - Recent history (rerun)
 * - Dialect switching
 *
 * The palette is a render-prop of `LabLayout`; opening it is a global
 * ⌘K keyboard shortcut. Selecting an item dispatches an action via
 * the provided callbacks.
 */

import { useEffect, useState } from "react";
import { BookOpen, Clock, Globe2, History, Star } from "lucide-react";
import { useNavigate } from "react-router-dom";

import {
  ProductCommandPalette,
  ProductCommandPaletteGroup as PaletteGroup,
  ProductCommandPaletteItem as Item,
} from "@/components/product/ProductCommandPalette";
import { availableAdminDestinations } from "@/app/routing/adminManifest";
import { useBackendInfo } from "@/features/admin/backend/queries";
import {
  type Dialect,
  useWorkbenchStore,
} from "@/features/admin/workbench/store";
import { compactQuery, formatRelative } from "@/lib/utils";
import { API_DOCS_PATH } from "@/lib/routes";

export interface CommandPaletteProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onLoadQuery: (dialect: Dialect, query: string) => void;
  onSwitchDialect: (dialect: Dialect) => void;
}

export function CommandPalette({
  open,
  onOpenChange,
  onLoadQuery,
  onSwitchDialect,
}: CommandPaletteProps) {
  const saved = useWorkbenchStore((s) => s.saved);
  const history = useWorkbenchStore((s) => s.history);
  const navigate = useNavigate();
  const { data: info } = useBackendInfo();
  const destinations = availableAdminDestinations(info?.capabilities);
  const queryDestinations = destinations.filter(
    (destination) => destination.palette === "query"
  );
  const goToDestinations = destinations.filter(
    (destination) => destination.palette === "go-to"
  );

  const goTo = (path: string) => {
    navigate(path);
    onOpenChange(false);
  };

  const openReference = () => {
    window.open(API_DOCS_PATH, "_blank", "noopener,noreferrer");
    onOpenChange(false);
  };

  const [value, setValue] = useState("");
  useEffect(() => {
    if (!open) setValue("");
  }, [open]);

  return (
    <ProductCommandPalette
      open={open}
      onOpenChange={onOpenChange}
      value={value}
      onValueChange={setValue}
      eyebrow="Admin control plane"
      description="Navigate tools, queries, and recent work"
      placeholder="Search commands, destinations, and queries…"
      indexLabel="Command index"
      emptyDescription="Try a tool name, destination, or saved query."
    >
      <PaletteGroup heading="Query" tone="promoter">
        {queryDestinations.map((destination) => {
          const Icon = destination.icon;
          const dialect = destination.id === "sql" ? "sql" : "sparql";
          return (
            <Item
              key={destination.id}
              icon={<Icon size={14} />}
              label={destination.paletteLabel ?? destination.label}
              onSelect={() => {
                onSwitchDialect(dialect);
                onOpenChange(false);
              }}
            />
          );
        })}
      </PaletteGroup>

      <PaletteGroup heading="Go to" tone="rbs">
        {goToDestinations.map((destination) => {
          const Icon = destination.icon;
          return (
            <Item
              key={destination.id}
              icon={<Icon size={14} />}
              label={destination.paletteLabel ?? destination.label}
              onSelect={() => goTo(destination.path)}
            />
          );
        })}
      </PaletteGroup>

      <PaletteGroup heading="Product" tone="cds">
        <Item
          icon={<Globe2 size={14} />}
          label="Open public registry"
          trailing="Registry"
          onSelect={() => goTo("/")}
        />
        <Item
          icon={<BookOpen size={14} />}
          label="Open API reference"
          trailing="New tab"
          onSelect={openReference}
        />
      </PaletteGroup>

      {saved.length > 0 && (
        <PaletteGroup heading="Saved queries" tone="cds">
          {saved.map((q) => (
            <Item
              key={q.id}
              icon={<Star size={14} />}
              label={q.name}
              trailing={q.dialect.toUpperCase()}
              onSelect={() => {
                onLoadQuery(q.dialect, q.query);
                onOpenChange(false);
              }}
            />
          ))}
        </PaletteGroup>
      )}

      {history.length > 0 && (
        <PaletteGroup heading="History" tone="terminator">
          {history.slice(0, 20).map((h) => (
            <Item
              key={h.id}
              icon={
                h.ok ? (
                  <History size={14} />
                ) : (
                  <Clock size={14} className="text-destructive" />
                )
              }
              label={compactQuery(h.query)}
              mono
              trailing={`${h.dialect.toUpperCase()} · ${formatRelative(h.ranAt)}`}
              onSelect={() => {
                onLoadQuery(h.dialect, h.query);
                onOpenChange(false);
              }}
            />
          ))}
        </PaletteGroup>
      )}
    </ProductCommandPalette>
  );
}
