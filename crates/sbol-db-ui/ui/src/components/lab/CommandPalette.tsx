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
import { Command } from "cmdk";
import {
  Activity,
  Clock,
  Compass,
  Database,
  Gauge,
  HardDrive,
  History,
  Home,
  Library,
  Network,
  Star,
  Table2,
} from "lucide-react";
import { useNavigate } from "react-router-dom";

import { type Dialect, useLabStore } from "@/lib/store";

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

  const goTo = (path: string) => {
    navigate(path);
    onOpenChange(false);
  };

  const [value, setValue] = useState("");
  useEffect(() => {
    if (!open) setValue("");
  }, [open]);

  if (!open) return null;

  return (
    <div
      role="dialog"
      aria-modal="true"
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/60 px-4 pt-24 backdrop-blur-sm"
      onClick={() => onOpenChange(false)}
    >
      <Command
        label="Command palette"
        className="w-full max-w-xl overflow-hidden rounded-lg border bg-popover text-popover-foreground shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        loop
      >
        <Command.Input
          autoFocus
          placeholder="Type a command or search…"
          value={value}
          onValueChange={setValue}
          className="w-full border-0 border-b bg-transparent px-4 py-3 text-sm text-foreground outline-none placeholder:text-muted-foreground"
        />
        <Command.List className="max-h-[60vh] overflow-y-auto py-1">
          <Command.Empty className="px-4 py-3 text-sm text-muted-foreground">
            No matches.
          </Command.Empty>

          <Command.Group
            heading="Dialect"
            className="px-2 py-1 text-[10px] uppercase tracking-wider text-muted-foreground"
          >
            <Item
              icon={<Network size={14} />}
              label="Switch to SPARQL"
              onSelect={() => {
                onSwitchDialect("sparql");
                onOpenChange(false);
              }}
            />
            <Item
              icon={<Database size={14} />}
              label="Switch to SQL"
              onSelect={() => {
                onSwitchDialect("sql");
                onOpenChange(false);
              }}
            />
          </Command.Group>

          <Command.Group
            heading="Go to"
            className="px-2 py-1 text-[10px] uppercase tracking-wider text-muted-foreground"
          >
            <Item icon={<Home size={14} />} label="Overview" onSelect={() => goTo("/")} />
            <Item
              icon={<Table2 size={14} />}
              label="Schema browser"
              onSelect={() => goTo("/schema")}
            />
            <Item
              icon={<Library size={14} />}
              label="Ontologies"
              onSelect={() => goTo("/ontologies")}
            />
            <Item
              icon={<Activity size={14} />}
              label="Observability"
              trailing="metrics + jobs"
              onSelect={() => goTo("/observability")}
            />
            <Item
              icon={<Gauge size={14} />}
              label="App metrics"
              onSelect={() => goTo("/observability")}
            />
            <Item
              icon={<HardDrive size={14} />}
              label="Postgres Maintenance"
              onSelect={() => goTo("/observability/postgres")}
            />
            <Item
              icon={<Compass size={14} />}
              label="Explore"
              onSelect={() => goTo("/schema")}
            />
          </Command.Group>

          {saved.length > 0 && (
            <Command.Group
              heading="Saved queries"
              className="px-2 py-1 text-[10px] uppercase tracking-wider text-muted-foreground"
            >
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
            </Command.Group>
          )}

          {history.length > 0 && (
            <Command.Group
              heading="History"
              className="px-2 py-1 text-[10px] uppercase tracking-wider text-muted-foreground"
            >
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
                  label={firstLine(h.query)}
                  trailing={`${h.dialect.toUpperCase()} · ${h.elapsedMs} ms`}
                  onSelect={() => {
                    onLoadQuery(h.dialect, h.query);
                    onOpenChange(false);
                  }}
                />
              ))}
            </Command.Group>
          )}
        </Command.List>
        <div className="flex items-center gap-3 border-t px-3 py-1.5 font-mono text-[10px] text-muted-foreground">
          <kbd>↑↓</kbd> navigate
          <kbd>↵</kbd> select
          <kbd>esc</kbd> close
        </div>
      </Command>
    </div>
  );
}

function Item({
  icon,
  label,
  trailing,
  onSelect,
}: {
  icon: React.ReactNode;
  label: string;
  trailing?: string;
  onSelect: () => void;
}) {
  return (
    <Command.Item
      onSelect={onSelect}
      className="mx-1 flex cursor-pointer items-center gap-2 rounded-md px-3 py-2 text-sm text-foreground aria-selected:bg-accent"
    >
      <span className="text-muted-foreground">{icon}</span>
      <span className="flex-1 truncate">{label}</span>
      {trailing && (
        <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
          {trailing}
        </span>
      )}
    </Command.Item>
  );
}

function firstLine(q: string): string {
  const line = q.split("\n").find((l) => l.trim().length > 0) ?? q;
  return line.length > 80 ? `${line.slice(0, 80)}…` : line;
}
