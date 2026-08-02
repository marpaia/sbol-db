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
import {
  BookOpen,
  Boxes,
  Clock,
  Database,
  Dna,
  Gauge,
  GitBranch,
  Globe2,
  HardDrive,
  History,
  Home,
  Import,
  Library,
  Network,
  Share2,
  Star,
  Table2,
} from "lucide-react";
import { useNavigate } from "react-router-dom";

import {
  ProductCommandPalette,
  ProductCommandPaletteGroup as PaletteGroup,
  ProductCommandPaletteItem as Item,
} from "@/components/product/ProductCommandPalette";
import { useBackendInfo } from "@/hooks/useBackendInfo";
import { type Dialect, useLabStore } from "@/lib/store";
import { adminPath } from "@/lib/routes";
import { compactQuery, formatRelative } from "@/lib/utils";

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
  const saved = useLabStore((s) => s.saved);
  const history = useLabStore((s) => s.history);
  const navigate = useNavigate();
  const { data: info } = useBackendInfo();
  const sqlConsole = info?.capabilities.sql_console ?? false;
  const hasMaintenance = (info?.capabilities.maintenance ?? null) !== null;

  const goTo = (path: string) => {
    navigate(path);
    onOpenChange(false);
  };

  const openReference = () => {
    window.open("/api/v2/docs", "_blank", "noopener,noreferrer");
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
        <Item
          icon={<Network size={14} />}
          label="SPARQL"
          onSelect={() => {
            onSwitchDialect("sparql");
            onOpenChange(false);
          }}
        />
        {sqlConsole && (
          <Item
            icon={<Database size={14} />}
            label="SQL"
            onSelect={() => {
              onSwitchDialect("sql");
              onOpenChange(false);
            }}
          />
        )}
      </PaletteGroup>

      <PaletteGroup heading="Go to" tone="rbs">
        <Item
          icon={<Home size={14} />}
          label="Overview"
          onSelect={() => goTo(adminPath())}
        />
        <Item
          icon={<Share2 size={14} />}
          label="Graphs"
          onSelect={() => goTo(adminPath("/graphs"))}
        />
        <Item
          icon={<Import size={14} />}
          label="Import"
          onSelect={() => goTo(adminPath("/import"))}
        />
        <Item
          icon={<Boxes size={14} />}
          label="Objects"
          onSelect={() => goTo(adminPath("/objects"))}
        />
        <Item
          icon={<Boxes size={14} />}
          label="Bulk object lookup"
          onSelect={() => goTo(adminPath("/objects/lookup"))}
        />
        <Item
          icon={<GitBranch size={14} />}
          label="Walk neighborhood"
          onSelect={() => goTo(adminPath("/neighborhood"))}
        />
        <Item
          icon={<Dna size={14} />}
          label="Sequence search"
          onSelect={() => goTo(adminPath("/sequences"))}
        />
        <Item
          icon={<Library size={14} />}
          label="Ontologies"
          onSelect={() => goTo(adminPath("/ontologies"))}
        />
        <Item
          icon={<Table2 size={14} />}
          label="Schema"
          onSelect={() => goTo(adminPath("/schema"))}
        />
        <Item
          icon={<Gauge size={14} />}
          label="Metrics"
          onSelect={() => goTo(adminPath("/observability"))}
        />
        {hasMaintenance && (
          <Item
            icon={<HardDrive size={14} />}
            label="Maintenance"
            onSelect={() => goTo(adminPath("/observability/maintenance"))}
          />
        )}
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
