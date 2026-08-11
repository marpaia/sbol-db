/**
 * Admin dashboard backed by the universal RDF catalog.
 * and lays it out as a one-screen data dashboard:
 *
 *  - Corpus counts (objects, graphs, triples, sequences, …)
 *  - Top RDF classes by resource count
 *  - Named graphs
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
  GitGraph,
  Library,
  Network,
  Play,
  Plus,
  Share2,
} from "lucide-react";

import { AdminPage } from "@/components/admin/AdminPage";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { OntologyLoaderDialog } from "@/components/lab/OntologyLoaderDialog";
import { KpiTile } from "@/components/observability/KpiTile";
import {
  ProductSurface,
  ProductSurfaceBody,
  ProductSurfaceHeader,
} from "@/components/product/ProductSurface";
import { overviewKeys, useOverview } from "@/features/admin/overview/queries";
import { useBackendInfo } from "@/features/admin/backend/queries";
import { schemaKeys } from "@/features/admin/schema/queries";
import {
  type Dialect,
  useWorkbenchStore,
} from "@/features/admin/workbench/store";
import { adminPath } from "@/lib/routes";

export default function DashboardRoute() {
  const { data, isLoading, error } = useOverview();
  const { data: backend } = useBackendInfo();
  const navigate = useNavigate();
  const setBuffer = useWorkbenchStore((s) => s.setBuffer);
  const queryClient = useQueryClient();
  const [loaderOpen, setLoaderOpen] = useState(false);

  const launch = useCallback(
    (dialect: Dialect, query: string) => {
      setBuffer(dialect, query);
      navigate(adminPath(`/${dialect}`));
    },
    [navigate, setBuffer]
  );

  const onLoaded = useCallback(() => {
    queryClient.invalidateQueries({ queryKey: overviewKeys.all });
    queryClient.invalidateQueries({ queryKey: schemaKeys.sparql() });
  }, [queryClient]);

  if (error) {
    return (
      <AdminPage
        title="Registry overview"
        description="Corpus health and operational entry points for this deployment."
      >
        <ErrorBanner
          title="Couldn't load the overview"
          body={(error as Error).message}
        />
      </AdminPage>
    );
  }

  const c = data?.counts;
  return (
    <>
      <AdminPage
        title="Registry overview"
        eyebrow="Admin control plane"
        description="Inspect the loaded corpus, check its operational shape, and move into focused data or query tools."
        maxWidth="7xl"
      >
        <section>
          <SectionLabel>Corpus</SectionLabel>
          <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-5 gap-3">
            <KpiTile
              icon={Boxes}
              label="Resources"
              value={c?.resources}
              loading={isLoading}
            />
            <KpiTile
              icon={Share2}
              label="Graphs"
              value={c?.named_graphs}
              loading={isLoading}
            />
            <KpiTile
              icon={GitGraph}
              label="Triples"
              value={c?.triples}
              loading={isLoading}
            />
            <KpiTile
              icon={Database}
              label="Sequences"
              value={c?.sequences}
              loading={isLoading}
            />
            <KpiTile
              icon={Library}
              label="Loaded ontologies"
              value={c?.ontologies}
              loading={isLoading}
            />
          </div>
        </section>

        <div className="grid lg:grid-cols-2 gap-6">
          <Panel
            title="Top RDF classes"
            subtitle={
              data && data.top_classes.length > 0
                ? `${data.top_classes.length} classes in use`
                : undefined
            }
          >
            {isLoading ? (
              <Skeleton lines={4} />
            ) : data?.top_classes.length === 0 ? (
              <Empty>No typed RDF resource classes are available yet.</Empty>
            ) : (
              <ul className="divide-y">
                {data?.top_classes.map((cls) => (
                  <li
                    key={cls.iri}
                    className="flex items-center gap-3 py-2 text-sm"
                  >
                    <button
                      type="button"
                      onClick={() => launch("sparql", classQuery(cls.iri))}
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
          title="Named graphs"
          subtitle={data ? `${data.graphs.length} shown` : undefined}
        >
          {isLoading ? (
            <Skeleton lines={3} />
          ) : data?.graphs.length === 0 ? (
            <Empty>No graphs yet.</Empty>
          ) : (
            <ul className="divide-y">
              {data?.graphs.map((d) => (
                <li key={d.id}>
                  <Link
                    to={adminPath(`/graphs/${d.id}`)}
                    className="block py-2 text-sm transition-colors hover:bg-accent/40"
                  >
                    <div className="flex items-center gap-3">
                      <span className="flex-1 truncate font-mono text-foreground">
                        {displayName(d)}
                      </span>
                      <span className="text-xs tabular-nums text-muted-foreground">
                        {d.triple_count === null
                          ? "RDF graph"
                          : `${d.triple_count.toLocaleString()} triples`}
                      </span>
                      <span className="w-28 shrink-0 text-right text-xs text-muted-foreground">
                        {d.created_at
                          ? formatRelative(d.created_at)
                          : "Not catalogued"}
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
              title="Typed RDF resources"
              description="SELECT resources, their RDF classes, and any available label."
              onClick={() =>
                launch(
                  "sparql",
                  `PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX dcterms: <http://purl.org/dc/terms/>\nPREFIX sbol2: <http://sbols.org/v2#>\nPREFIX sbol3: <http://sbols.org/v3#>\nSELECT ?resource ?class ?name WHERE {\n  ?resource rdf:type ?class .\n  OPTIONAL {\n    VALUES ?labelPredicate { rdfs:label dcterms:title sbol2:name sbol3:name }\n    ?resource ?labelPredicate ?name .\n  }\n}\nLIMIT 50\n`
                )
              }
            />
            <Template
              dialect="sparql"
              icon={<Network className="size-3.5" />}
              title="Resources by RDF class"
              description="Count distinct resources for every RDF class."
              onClick={() =>
                launch(
                  "sparql",
                  `SELECT ?class (COUNT(DISTINCT ?resource) AS ?resources) WHERE {\n  ?resource a ?class .\n}\nGROUP BY ?class\nORDER BY DESC(?resources)\n`
                )
              }
            />
            {backend?.capabilities.sql_console && (
              <>
                <Template
                  dialect="sql"
                  icon={<Database className="size-3.5" />}
                  title="Resources per RDF class"
                  description="Distribution of RDF class across the resource projection."
                  onClick={() =>
                    launch(
                      "sql",
                      `SELECT sbol_class, count(*) AS resources\nFROM sbol_objects\nGROUP BY sbol_class\nORDER BY resources DESC;\n`
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
              </>
            )}
          </div>
        </section>
      </AdminPage>
      <OntologyLoaderDialog
        open={loaderOpen}
        onOpenChange={setLoaderOpen}
        onLoaded={onLoaded}
        loadedPrefixes={data?.loaded_ontologies.map((o) => o.prefix) ?? []}
      />
    </>
  );
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <h2 className="ledger-label mb-3 text-muted-foreground">{children}</h2>
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
    <ProductSurface density="compact">
      <ProductSurfaceHeader
        density="compact"
        title={title}
        description={subtitle}
        action={action}
      />
      <ProductSurfaceBody density="compact" className="py-2">
        {children}
      </ProductSurfaceBody>
    </ProductSurface>
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

function classQuery(iri: string): string {
  return `PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX dcterms: <http://purl.org/dc/terms/>\nPREFIX sbol2: <http://sbols.org/v2#>\nPREFIX sbol3: <http://sbols.org/v3#>\nSELECT ?resource ?name WHERE {\n  ?resource a <${iri}> .\n  OPTIONAL {\n    VALUES ?labelPredicate { rdfs:label dcterms:title sbol2:name sbol3:name }\n    ?resource ?labelPredicate ?name .\n  }\n}\nLIMIT 25\n`;
}

function displayName(d: {
  name: string | null;
  source_uri: string | null;
  iri: string;
  id: string;
}): string {
  if (d.name) return d.name;
  if (d.source_uri) {
    const m = d.source_uri.match(/[^/]+$/);
    if (m) return m[0];
    return d.source_uri;
  }
  if (d.iri) return d.iri;
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
