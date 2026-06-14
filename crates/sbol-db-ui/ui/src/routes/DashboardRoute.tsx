/**
 * Landing page for the lab. Pulls everything from /lab/api/overview
 * and lays it out as a one-screen data dashboard:
 *
 *  - Corpus counts (objects, documents, quads, sequences, …)
 *  - Top SBOL classes by row count
 *  - Recent document imports
 *  - Loaded ontologies
 *  - Quick-start query templates for SPARQL and SQL
 *
 * Clicking a template loads the query into the appropriate buffer
 * and navigates to the workbench — the user lands on a useful page
 * and is one click from running something real.
 */

import { useCallback, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "react-router-dom";
import {
  Boxes,
  Database,
  FileText,
  GitGraph,
  Library,
  Network,
  Play,
  Plus,
  ShieldCheck,
} from "lucide-react";

import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { OntologyLoaderDialog } from "@/components/lab/OntologyLoaderDialog";
import { useOverview } from "@/hooks/useOverview";
import { type Dialect, useLabStore } from "@/lib/store";
import { cn } from "@/lib/utils";

function greeting(): string {
  const h = new Date().getHours();
  return h < 12 ? "Good morning" : h < 18 ? "Good afternoon" : "Good evening";
}

export default function DashboardRoute() {
  const { data, isLoading, error } = useOverview();
  const navigate = useNavigate();
  const setBuffer = useLabStore((s) => s.setBuffer);
  const queryClient = useQueryClient();
  const [loaderOpen, setLoaderOpen] = useState(false);

  const launch = useCallback(
    (dialect: Dialect, query: string) => {
      setBuffer(dialect, query);
      navigate(`/${dialect}`);
    },
    [navigate, setBuffer]
  );

  const onLoaded = useCallback(() => {
    queryClient.invalidateQueries({ queryKey: ["lab", "overview"] });
    queryClient.invalidateQueries({ queryKey: ["lab", "schema", "sparql"] });
  }, [queryClient]);

  if (error) {
    return (
      <div className="p-6">
        <ErrorBanner
          title="Couldn't load the overview"
          body={(error as Error).message}
        />
      </div>
    );
  }

  const c = data?.counts;

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="mx-auto max-w-6xl px-8 py-10 space-y-10">
        <header>
          <h1 className="text-2xl font-semibold tracking-tight">
            {greeting()}! 👋
          </h1>
          <p className="mt-2 text-sm text-muted-foreground">
            Welcome to the SBOL Data Lab. Query your corpus with SPARQL or SQL,
            browse the schema, or load ontology packs. The panels below show
            what's loaded, with a few templates to get you started.
          </p>
          <p className="mt-2 text-sm text-muted-foreground">
            This UI is powered by sbol-db, a Postgres-backed data management
            system for synthetic biology data. Check out sbol-db on{" "}
            <a
              href="https://github.com/marpaia/sbol-db"
              target="_blank"
              rel="noopener noreferrer"
              className="underline underline-offset-2 transition-colors hover:text-foreground"
            >
              GitHub
            </a>
            !
          </p>
        </header>

        <section>
          <SectionLabel>Corpus</SectionLabel>
          <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-6 gap-3">
            <CountCard
              icon={<Boxes className="size-4" />}
              label="Objects"
              value={c?.objects}
              loading={isLoading}
            />
            <CountCard
              icon={<FileText className="size-4" />}
              label="Documents"
              value={c?.documents}
              loading={isLoading}
            />
            <CountCard
              icon={<GitGraph className="size-4" />}
              label="Quads"
              value={c?.quads}
              loading={isLoading}
            />
            <CountCard
              icon={<Database className="size-4" />}
              label="Sequences"
              value={c?.sequences}
              loading={isLoading}
            />
            <CountCard
              icon={<ShieldCheck className="size-4" />}
              label="Validation runs"
              value={c?.validation_runs}
              loading={isLoading}
            />
            <CountCard
              icon={<Library className="size-4" />}
              label="Ontologies"
              value={c?.ontologies}
              loading={isLoading}
            />
          </div>
        </section>

        <div className="grid lg:grid-cols-2 gap-6">
          <Panel
            title="Top SBOL classes"
            subtitle={
              data && data.top_classes.length > 0
                ? `${data.top_classes.length} classes in use`
                : undefined
            }
          >
            {isLoading ? (
              <Skeleton lines={4} />
            ) : data?.top_classes.length === 0 ? (
              <Empty>No SBOL objects in the database yet.</Empty>
            ) : (
              <ul className="divide-y">
                {data?.top_classes.map((cls) => (
                  <li
                    key={cls.iri}
                    className="flex items-center gap-3 py-2 text-sm"
                  >
                    <button
                      type="button"
                      onClick={() =>
                        launch(
                          "sparql",
                          `PREFIX sbol: <http://sbols.org/v3#>\nSELECT ?s ?name WHERE {\n  ?s a <${cls.iri}> .\n  OPTIONAL { ?s sbol:name ?name }\n}\nLIMIT 25\n`
                        )
                      }
                      className="flex-1 truncate text-left font-mono text-foreground hover:underline"
                      title={`Query for ${cls.iri}`}
                    >
                      {shortIri(cls.iri)}
                    </button>
                    <span className="tabular-nums text-muted-foreground">
                      {cls.count.toLocaleString()}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </Panel>

          <Panel
            title="Loaded ontologies"
            subtitle={
              data && data.loaded_ontologies.length > 0
                ? `${data.loaded_ontologies.length} loaded`
                : undefined
            }
            action={
              <button
                type="button"
                onClick={() => setLoaderOpen(true)}
                className="inline-flex items-center gap-1 rounded-md border bg-background px-2 py-0.5 text-[11px] text-foreground transition-colors hover:bg-accent"
              >
                <Plus size={12} />
                Load
              </button>
            }
          >
            {isLoading ? (
              <Skeleton lines={3} />
            ) : data?.loaded_ontologies.length === 0 ? (
              <Empty>
                None loaded yet. Click{" "}
                <button
                  type="button"
                  onClick={() => setLoaderOpen(true)}
                  className="font-medium text-foreground hover:underline"
                >
                  Load
                </button>{" "}
                to fetch SO, SBO, or any OBO ontology by URL.
              </Empty>
            ) : (
              <ul className="divide-y">
                {data?.loaded_ontologies.map((o) => (
                  <li
                    key={o.prefix}
                    className="flex items-center gap-3 py-2 text-sm"
                  >
                    <span className="shrink-0 font-mono text-foreground">
                      {o.prefix.toLowerCase()}
                    </span>
                    <span className="flex-1 truncate text-muted-foreground">
                      {o.name}
                    </span>
                    <span className="text-xs tabular-nums text-muted-foreground">
                      {o.term_count.toLocaleString()} terms
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </Panel>
        </div>

        <Panel
          title="Recent imports"
          subtitle={data ? `last ${data.recent_documents.length}` : undefined}
        >
          {isLoading ? (
            <Skeleton lines={3} />
          ) : data?.recent_documents.length === 0 ? (
            <Empty>No documents imported yet.</Empty>
          ) : (
            <ul className="divide-y">
              {data?.recent_documents.map((d) => (
                <li key={d.id}>
                  <Link
                    to={`/documents/${d.id}`}
                    className="block py-2 text-sm transition-colors hover:bg-accent/40"
                  >
                    <div className="flex items-center gap-3">
                      <span className="flex-1 truncate font-mono text-foreground">
                        {displayName(d)}
                      </span>
                      <span className="text-xs tabular-nums text-muted-foreground">
                        {d.object_count.toLocaleString()} objects
                      </span>
                      <span className="w-28 shrink-0 text-right text-xs text-muted-foreground">
                        {formatRelative(d.created_at)}
                      </span>
                    </div>
                    {d.source_uri && (
                      <div className="mt-0.5 truncate font-mono text-[11px] text-muted-foreground/70">
                        {d.source_uri}
                      </div>
                    )}
                  </Link>
                </li>
              ))}
            </ul>
          )}
        </Panel>

        <section>
          <SectionLabel>Quick start</SectionLabel>
          <div className="grid md:grid-cols-2 gap-3">
            <Template
              dialect="sparql"
              icon={<Network className="size-3.5" />}
              title="All components"
              description="SELECT every sbol:Component along with its name."
              onClick={() =>
                launch(
                  "sparql",
                  `PREFIX sbol: <http://sbols.org/v3#>\nSELECT ?component ?name WHERE {\n  ?component a sbol:Component .\n  OPTIONAL { ?component sbol:name ?name }\n}\nLIMIT 50\n`
                )
              }
            />
            <Template
              dialect="sparql"
              icon={<Network className="size-3.5" />}
              title="Component roles, counted"
              description="Group every component by its role IRI."
              onClick={() =>
                launch(
                  "sparql",
                  `PREFIX sbol: <http://sbols.org/v3#>\nSELECT ?role (count(?c) AS ?n) WHERE {\n  ?c a sbol:Component ;\n     sbol:hasRole ?role .\n}\nGROUP BY ?role\nORDER BY DESC(?n)\n`
                )
              }
            />
            <Template
              dialect="sql"
              icon={<Database className="size-3.5" />}
              title="Objects per SBOL class"
              description="Distribution of sbol_class across the projection table."
              onClick={() =>
                launch(
                  "sql",
                  `SELECT sbol_class, count(*) AS objects\nFROM sbol_objects\nGROUP BY sbol_class\nORDER BY objects DESC;\n`
                )
              }
            />
            <Template
              dialect="sql"
              icon={<Database className="size-3.5" />}
              title="Nucleotide sequences with length"
              description="Length and alphabet for every stored sequence."
              onClick={() =>
                launch(
                  "sql",
                  `SELECT s.object_id, o.iri, s.alphabet, s.length_bp\nFROM sbol_sequences s\nJOIN sbol_objects o ON o.id = s.object_id\nORDER BY s.length_bp DESC NULLS LAST\nLIMIT 50;\n`
                )
              }
            />
          </div>
        </section>
      </div>
      <OntologyLoaderDialog
        open={loaderOpen}
        onOpenChange={setLoaderOpen}
        onLoaded={onLoaded}
        loadedPrefixes={data?.loaded_ontologies.map((o) => o.prefix) ?? []}
      />
    </div>
  );
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <h2 className="mb-3 text-[11px] font-medium uppercase tracking-wider text-muted-foreground">
      {children}
    </h2>
  );
}

function CountCard({
  icon,
  label,
  value,
  loading,
}: {
  icon: React.ReactNode;
  label: string;
  value: number | undefined;
  loading: boolean;
}) {
  return (
    <div className="rounded-lg border bg-card p-4 transition-colors hover:border-primary/40">
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <span className="text-primary">{icon}</span>
        <span>{label}</span>
      </div>
      <div
        className={cn(
          "mt-2 text-2xl font-semibold tabular-nums",
          loading ? "text-muted-foreground/40" : "text-foreground"
        )}
      >
        {loading || value === undefined ? "—" : value.toLocaleString()}
      </div>
    </div>
  );
}

function Panel({
  title,
  subtitle,
  action,
  children,
}: {
  title: string;
  subtitle?: string;
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="overflow-hidden rounded-lg border bg-card">
      <header className="flex items-center gap-2 border-b border-primary/15 bg-primary/5 px-4 py-2.5">
        <h3 className="text-sm font-medium text-primary">{title}</h3>
        {subtitle && (
          <span className="text-xs text-muted-foreground">{subtitle}</span>
        )}
        {action && <div className="ml-auto">{action}</div>}
      </header>
      <div className="px-4 py-2">{children}</div>
    </section>
  );
}

function Template({
  dialect,
  icon,
  title,
  description,
  onClick,
}: {
  dialect: Dialect;
  icon: React.ReactNode;
  title: string;
  description: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="group rounded-lg border bg-card p-4 text-left transition-colors hover:border-primary/40 hover:bg-accent"
    >
      <div className="flex items-center gap-2 text-[11px] font-medium uppercase tracking-wider text-primary">
        <span>{icon}</span>
        <span>{dialect}</span>
        <Play
          size={12}
          className="ml-auto text-muted-foreground/40 transition-colors group-hover:text-primary"
        />
      </div>
      <div className="mt-2 text-sm font-medium text-foreground">{title}</div>
      <div className="mt-1 text-xs text-muted-foreground">{description}</div>
    </button>
  );
}

function Skeleton({ lines }: { lines: number }) {
  return (
    <ul className="divide-y">
      {Array.from({ length: lines }).map((_, i) => (
        <li key={i} className="flex items-center gap-3 py-2">
          <span className="h-3 flex-1 animate-pulse rounded bg-muted" />
          <span className="h-3 w-12 animate-pulse rounded bg-muted" />
        </li>
      ))}
    </ul>
  );
}

function Empty({ children }: { children: React.ReactNode }) {
  return <div className="py-3 text-sm text-muted-foreground">{children}</div>;
}

function shortIri(iri: string): string {
  const m = iri.match(/[#/]([^#/]+)$/);
  return m ? m[1] : iri;
}

function displayName(d: {
  name: string | null;
  source_uri: string | null;
  id: string;
}): string {
  if (d.name) return d.name;
  if (d.source_uri) {
    const m = d.source_uri.match(/[^/]+$/);
    if (m) return m[0];
    return d.source_uri;
  }
  return d.id.slice(0, 8);
}

function formatRelative(iso: string): string {
  const then = new Date(iso).getTime();
  const now = Date.now();
  const seconds = Math.floor((now - then) / 1000);
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}
