/**
 * Schema browser. Two surfaces in one page, switched via tabs:
 *
 *  - SQL: every table the lab exposes through PostgREST/SQL, with its
 *    columns, Postgres types, and nullability. Clicking a table copies
 *    a `SELECT * FROM <name> LIMIT 100` template into the SQL editor.
 *  - SPARQL: the prefix table (built-in + ontology-derived) and the
 *    top classes by row count. Clicking a class loads a SPARQL
 *    template against that IRI into the SPARQL editor.
 *
 * Data is sourced from the schema endpoints (`/lab/api/schema/sql`,
 * `/lab/api/schema/sparql`), cached via TanStack Query. There is no
 * server-side write here — this is a pure read-only browser.
 */

import { Link, useNavigate } from "react-router-dom";
import { ChevronRight, Copy, Database, Network, Play } from "lucide-react";

import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useSparqlSchema, useSqlSchema } from "@/hooks/useSchema";
import type {
  SparqlSchemaClass,
  SparqlSchemaPrefix,
  SqlSchemaColumn,
  SqlSchemaTable,
} from "@/lib/api";
import { type Dialect, useLabStore } from "@/lib/store";
import { cn } from "@/lib/utils";

export default function SchemaRoute() {
  const navigate = useNavigate();
  const setBuffer = useLabStore((s) => s.setBuffer);

  const launch = (dialect: Dialect, query: string) => {
    setBuffer(dialect, query);
    navigate(`/${dialect}`);
  };

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl space-y-6 px-8 py-10">
        <header>
          <h1 className="text-2xl font-semibold tracking-tight">Schema</h1>
          <p className="mt-2 text-sm text-muted-foreground">
            Read-only browser for the SQL projection and SPARQL prefix table.
            Click a table or class to drop a starter query into the editor.
          </p>
        </header>

        <Tabs defaultValue="sql" className="w-full">
          <TabsList>
            <TabsTrigger value="sql" className="gap-1.5">
              <Database className="size-3.5" />
              SQL
            </TabsTrigger>
            <TabsTrigger value="sparql" className="gap-1.5">
              <Network className="size-3.5" />
              SPARQL
            </TabsTrigger>
          </TabsList>

          <TabsContent value="sql">
            <SqlPanel onLaunch={(q) => launch("sql", q)} />
          </TabsContent>

          <TabsContent value="sparql">
            <SparqlPanel onLaunch={(q) => launch("sparql", q)} />
          </TabsContent>
        </Tabs>
      </div>
    </div>
  );
}

function SqlPanel({ onLaunch }: { onLaunch: (query: string) => void }) {
  const { data, isLoading, error } = useSqlSchema();

  if (error) {
    return (
      <ErrorBanner
        title="Couldn't load SQL schema"
        body={(error as Error).message}
      />
    );
  }

  if (isLoading || !data) {
    return <TableSkeleton />;
  }

  if (data.tables.length === 0) {
    return (
      <Empty>No tables available — the projection layer may be empty.</Empty>
    );
  }

  return (
    <div className="space-y-3">
      {data.tables.map((t) => (
        <TableCard key={t.name} table={t} onLaunch={onLaunch} />
      ))}
    </div>
  );
}

function TableCard({
  table,
  onLaunch,
}: {
  table: SqlSchemaTable;
  onLaunch: (query: string) => void;
}) {
  const template = `SELECT *\nFROM ${table.name}\nLIMIT 100;\n`;
  const detailHref = `/schema/tables/public/${encodeURIComponent(table.name)}`;
  return (
    <section className="rounded-lg border bg-card">
      <header className="flex items-center gap-2 border-b px-2 py-1.5">
        <Link
          to={detailHref}
          title={`Open ${table.name} stats and indexes`}
          className="group flex min-w-0 flex-1 items-center gap-3 rounded-md px-2 py-1 transition-colors hover:bg-accent"
        >
          <Database
            size={14}
            className="shrink-0 text-muted-foreground/70"
            aria-hidden
          />
          <h3 className="truncate font-mono text-sm text-foreground">
            {table.name}
          </h3>
          <span className="shrink-0 text-xs text-muted-foreground">
            {table.columns.length}{" "}
            {table.columns.length === 1 ? "column" : "columns"}
          </span>
          <ChevronRight
            size={12}
            className="ml-auto shrink-0 text-muted-foreground/40 transition-all group-hover:translate-x-0.5 group-hover:text-foreground"
            aria-hidden
          />
        </Link>
        <button
          type="button"
          onClick={() => onLaunch(template)}
          className="inline-flex shrink-0 items-center gap-1 rounded-md border bg-background px-2 py-1 text-xs text-foreground transition-colors hover:bg-accent"
          title={`Insert "SELECT * FROM ${table.name}" into the SQL editor`}
        >
          <Play size={11} />
          Query
        </button>
      </header>
      <div className="grid grid-cols-1 gap-x-6 gap-y-1 px-4 py-3 sm:grid-cols-2 lg:grid-cols-3">
        {table.columns.map((c) => (
          <ColumnRow key={c.name} column={c} />
        ))}
      </div>
    </section>
  );
}

function ColumnRow({ column }: { column: SqlSchemaColumn }) {
  const copy = () => {
    if (typeof navigator !== "undefined" && navigator.clipboard) {
      navigator.clipboard.writeText(column.name).catch(() => {});
    }
  };
  return (
    <button
      type="button"
      onClick={copy}
      title={`Copy "${column.name}" to clipboard`}
      className="group flex min-w-0 items-baseline gap-2 rounded-md px-1.5 py-1 text-left text-sm transition-colors hover:bg-accent"
    >
      <span className="truncate font-mono text-foreground">{column.name}</span>
      <span className="ml-auto shrink-0 font-mono text-[10px] uppercase tracking-wide text-muted-foreground">
        {column.pg_type}
      </span>
      {column.nullable && (
        <span
          className="shrink-0 text-[10px] text-muted-foreground/60"
          title="nullable"
        >
          null
        </span>
      )}
      <Copy
        size={11}
        className="shrink-0 text-muted-foreground/0 transition-colors group-hover:text-muted-foreground"
        aria-hidden
      />
    </button>
  );
}

function SparqlPanel({ onLaunch }: { onLaunch: (query: string) => void }) {
  const { data, isLoading, error } = useSparqlSchema();

  if (error) {
    return (
      <ErrorBanner
        title="Couldn't load SPARQL schema"
        body={(error as Error).message}
      />
    );
  }

  if (isLoading || !data) {
    return <TableSkeleton />;
  }

  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <Panel
        title="Prefixes"
        subtitle={
          data.prefixes.length > 0 ? `${data.prefixes.length} known` : undefined
        }
      >
        {data.prefixes.length === 0 ? (
          <Empty>No prefixes registered.</Empty>
        ) : (
          <ul className="divide-y">
            {data.prefixes.map((p) => (
              <PrefixRow key={p.prefix} prefix={p} />
            ))}
          </ul>
        )}
      </Panel>

      <Panel
        title="Top classes"
        subtitle={
          data.top_classes.length > 0
            ? `${data.top_classes.length} in use`
            : undefined
        }
      >
        {data.top_classes.length === 0 ? (
          <Empty>No SBOL objects in the database yet.</Empty>
        ) : (
          <ul className="divide-y">
            {data.top_classes.map((c) => (
              <ClassRow key={c.iri} cls={c} onLaunch={onLaunch} />
            ))}
          </ul>
        )}
      </Panel>
    </div>
  );
}

function PrefixRow({ prefix }: { prefix: SparqlSchemaPrefix }) {
  const copy = () => {
    if (typeof navigator !== "undefined" && navigator.clipboard) {
      navigator.clipboard.writeText(prefix.iri).catch(() => {});
    }
  };
  return (
    <li className="flex items-center gap-3 py-2 text-sm">
      <button
        type="button"
        onClick={copy}
        title={`Copy ${prefix.iri}`}
        className="group flex flex-1 min-w-0 items-center gap-3 rounded-md px-1.5 py-0.5 text-left transition-colors hover:bg-accent"
      >
        <span className="w-16 shrink-0 font-mono text-foreground">
          {prefix.prefix}
        </span>
        <span className="flex-1 truncate font-mono text-xs text-muted-foreground">
          {prefix.iri}
        </span>
      </button>
      {prefix.from_ontology && (
        <span className="shrink-0 rounded-sm bg-primary/10 px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-primary">
          ontology
        </span>
      )}
    </li>
  );
}

function ClassRow({
  cls,
  onLaunch,
}: {
  cls: SparqlSchemaClass;
  onLaunch: (query: string) => void;
}) {
  const template = `PREFIX sbol: <http://sbols.org/v3#>\nSELECT ?s ?name WHERE {\n  ?s a <${cls.iri}> .\n  OPTIONAL { ?s sbol:name ?name }\n}\nLIMIT 25\n`;
  return (
    <li className="flex items-center gap-3 py-2 text-sm">
      <button
        type="button"
        onClick={() => onLaunch(template)}
        title={`Insert SPARQL template for ${cls.iri}`}
        className="group flex flex-1 min-w-0 items-center gap-2 rounded-md px-1.5 py-0.5 text-left transition-colors hover:bg-accent"
      >
        <span className="truncate font-mono text-foreground">
          {shortIri(cls.iri)}
        </span>
        <Play
          size={11}
          className="shrink-0 text-muted-foreground/0 transition-colors group-hover:text-muted-foreground"
          aria-hidden
        />
      </button>
      <span className="shrink-0 tabular-nums text-muted-foreground">
        {cls.count.toLocaleString()}
      </span>
    </li>
  );
}

function Panel({
  title,
  subtitle,
  children,
}: {
  title: string;
  subtitle?: string;
  children: React.ReactNode;
}) {
  return (
    <section className="rounded-lg border bg-card">
      <header className="flex items-center gap-2 border-b px-4 py-2.5">
        <h3 className="text-sm font-medium">{title}</h3>
        {subtitle && (
          <span className="text-xs text-muted-foreground">{subtitle}</span>
        )}
      </header>
      <div className="px-4 py-2">{children}</div>
    </section>
  );
}

function TableSkeleton() {
  return (
    <div className="space-y-3">
      {Array.from({ length: 3 }).map((_, i) => (
        <div
          key={i}
          className={cn(
            "h-32 animate-pulse rounded-lg border bg-card",
            i === 0 ? "opacity-100" : i === 1 ? "opacity-70" : "opacity-40"
          )}
        />
      ))}
    </div>
  );
}

function Empty({ children }: { children: React.ReactNode }) {
  return <div className="py-3 text-sm text-muted-foreground">{children}</div>;
}

function shortIri(iri: string): string {
  const m = iri.match(/[#/]([^#/]+)$/);
  return m ? m[1] : iri;
}
