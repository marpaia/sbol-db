/**
 * Left-rail schema browser. Renders a collapsible tree of tables /
 * columns for SQL, and a list of prefixes + top classes for SPARQL.
 * Clicking a node calls `onInsert` with the text to splice into the
 * editor — the parent routes own the buffer.
 */

import { useState } from "react";
import { ChevronDown, ChevronRight, Database, Network } from "lucide-react";

import { useSparqlSchema, useSqlSchema } from "@/hooks/useSchema";
import type { Dialect } from "@/lib/store";
import { cn } from "@/lib/utils";

export interface SchemaSidebarProps {
  dialect: Dialect;
  onInsert: (text: string) => void;
}

export function SchemaSidebar({ dialect, onInsert }: SchemaSidebarProps) {
  return (
    <div className="h-full w-full overflow-y-auto border-r bg-background">
      <div className="flex items-center gap-2 border-b px-3 py-2 text-[11px] uppercase tracking-wider text-muted-foreground">
        {dialect === "sql" ? (
          <>
            <Database size={12} /> Tables
          </>
        ) : (
          <>
            <Network size={12} /> Prefixes & classes
          </>
        )}
      </div>
      {dialect === "sql" ? (
        <SqlTree onInsert={onInsert} />
      ) : (
        <SparqlTree onInsert={onInsert} />
      )}
    </div>
  );
}

function SqlTree({ onInsert }: { onInsert: (s: string) => void }) {
  const { data, isLoading, error } = useSqlSchema();
  if (isLoading) return <Hint>loading…</Hint>;
  if (error) return <Hint tone="error">{(error as Error).message}</Hint>;
  if (!data || data.tables.length === 0) return <Hint>no tables</Hint>;
  return (
    <ul className="py-1">
      {data.tables.map((t) => (
        <TableNode
          key={t.name}
          name={t.name}
          columns={t.columns}
          onInsert={onInsert}
        />
      ))}
    </ul>
  );
}

function TableNode({
  name,
  columns,
  onInsert,
}: {
  name: string;
  columns: { name: string; pg_type: string; nullable: boolean }[];
  onInsert: (s: string) => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <li>
      <div className="group flex items-center px-2 py-1 text-sm transition-colors hover:bg-accent">
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          className="mr-1 text-muted-foreground/60"
          aria-label={open ? "collapse" : "expand"}
        >
          {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        </button>
        <button
          type="button"
          onClick={() => onInsert(name)}
          className="flex-1 truncate text-left font-mono text-foreground"
          title="Click to insert"
        >
          {name}
        </button>
        <span className="ml-2 text-[10px] text-muted-foreground/60 opacity-0 group-hover:opacity-100">
          {columns.length}
        </span>
      </div>
      {open && (
        <ul className="ml-5 border-l">
          {columns.map((c) => (
            <li key={c.name}>
              <button
                type="button"
                onClick={() => onInsert(c.name)}
                className="flex w-full items-center gap-2 px-2 py-0.5 text-left font-mono text-xs transition-colors hover:bg-accent"
                title={`${c.pg_type}${c.nullable ? " · nullable" : ""}`}
              >
                <span className="truncate text-foreground/90">{c.name}</span>
                <span className="ml-auto text-[10px] uppercase text-muted-foreground/70">
                  {c.pg_type}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </li>
  );
}

function SparqlTree({ onInsert }: { onInsert: (s: string) => void }) {
  const { data, isLoading, error } = useSparqlSchema();
  if (isLoading) return <Hint>loading…</Hint>;
  if (error) return <Hint tone="error">{(error as Error).message}</Hint>;
  if (!data) return null;
  return (
    <div className="py-2">
      <Section title="Prefixes">
        {data.prefixes.map((p) => (
          <button
            key={p.prefix}
            type="button"
            onClick={() => onInsert(`PREFIX ${p.prefix}: <${p.iri}>\n`)}
            className="flex w-full items-center gap-2 px-3 py-1 text-left font-mono text-xs transition-colors hover:bg-accent"
            title={p.iri}
          >
            <span className="shrink-0 text-foreground">{p.prefix}:</span>
            <span className="truncate text-muted-foreground">{p.iri}</span>
            {p.from_ontology && (
              <span className="ml-auto shrink-0 text-[10px] text-muted-foreground/70">
                onto
              </span>
            )}
          </button>
        ))}
      </Section>
      <Section title="Top classes">
        {data.top_classes.length === 0 ? (
          <Hint>no data yet</Hint>
        ) : (
          data.top_classes.map((c) => (
            <button
              key={c.iri}
              type="button"
              onClick={() => onInsert(`<${c.iri}>`)}
              className="flex w-full items-center gap-2 px-3 py-1 text-left font-mono text-xs transition-colors hover:bg-accent"
              title={c.iri}
            >
              <span className="truncate text-foreground/90">
                {shortIri(c.iri)}
              </span>
              <span className="ml-auto text-[10px] tabular-nums text-muted-foreground">
                {c.count}
              </span>
            </button>
          ))
        )}
      </Section>
    </div>
  );
}

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <div className="px-3 pb-1 pt-2 text-[11px] uppercase tracking-wider text-muted-foreground">
        {title}
      </div>
      <ul>{children}</ul>
    </div>
  );
}

function Hint({
  children,
  tone = "muted",
}: {
  children: React.ReactNode;
  tone?: "muted" | "error";
}) {
  return (
    <div
      className={cn(
        "px-3 py-2 font-mono text-xs",
        tone === "error" ? "text-destructive" : "text-muted-foreground"
      )}
    >
      {children}
    </div>
  );
}

function shortIri(iri: string): string {
  const m = iri.match(/[#/]([^#/]+)$/);
  return m ? `…${m[1]}` : iri;
}
