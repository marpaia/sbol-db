import {
  ArrowRight,
  Boxes,
  Cable,
  DatabaseZap,
  Dna,
  FilePlus2,
  FolderKanban,
  Search,
  ShieldCheck,
} from "lucide-react";
import { Link, useNavigate } from "react-router-dom";

import { ObjectResultCard } from "@/components/portal/ObjectResultCard";
import { MachineAccessSection } from "@/components/portal/MachineAccessSection";
import { SearchBox } from "@/components/portal/SearchBox";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import {
  useInstance,
  usePortalSearch,
  useSession,
} from "@/features/portal/queries";
import { PRODUCT_NAME } from "@/lib/product";

export default function HomeRoute() {
  const navigate = useNavigate();
  const instance = useInstance();
  const session = useSession();
  const recent = usePortalSearch({ limit: 6 });
  const user = session.data?.authenticated ? session.data.user : null;

  return (
    <>
      <section className="portal-hero-grid relative overflow-hidden border-b">
        <div className="pointer-events-none absolute inset-0 bg-[radial-gradient(circle_at_50%_15%,hsl(var(--primary)/0.16),transparent_42%)]" />
        <div className="relative mx-auto max-w-5xl px-4 py-20 text-center sm:px-6 sm:py-28 lg:px-8">
          <Badge className="mb-6 border-primary/15 bg-background/80 px-3 py-1 text-primary shadow-sm backdrop-blur">
            Open biological design infrastructure
          </Badge>
          <h1 className="mx-auto max-w-4xl text-balance text-4xl font-semibold tracking-[-0.035em] sm:text-6xl">
            Find, share, and reuse biological designs.
          </h1>
          <p className="mx-auto mt-6 max-w-2xl text-pretty text-base leading-7 text-muted-foreground sm:text-lg">
            Search {PRODUCT_NAME} for parts, systems, sequences, and
            collections. Every result keeps its SBOL identity, provenance, and
            machine-readable representation close at hand.
          </p>
          <SearchBox
            size="hero"
            className="mx-auto mt-9 max-w-3xl text-left"
            onSearch={(query) =>
              navigate(
                query ? `/search?q=${encodeURIComponent(query)}` : "/search"
              )
            }
          />
          <div className="mt-5 flex flex-wrap items-center justify-center gap-x-5 gap-y-2 text-xs text-muted-foreground">
            <span className="inline-flex items-center gap-1.5">
              <ShieldCheck className="size-3.5 text-primary" /> ACL-aware
              results
            </span>
            <span className="inline-flex items-center gap-1.5">
              <Boxes className="size-3.5 text-primary" /> SBOL 2 and SBOL 3
            </span>
            <span className="inline-flex items-center gap-1.5">
              <DatabaseZap className="size-3.5 text-primary" /> Open REST and
              RDF access
            </span>
          </div>
        </div>
      </section>

      {instance.data?.front_page_text && (
        <section className="border-b bg-muted/20">
          <div className="mx-auto max-w-4xl px-4 py-8 text-center text-sm leading-7 text-muted-foreground sm:px-6">
            {instance.data.front_page_text}
          </div>
        </section>
      )}

      <section className="mx-auto max-w-7xl px-4 py-14 sm:px-6 lg:px-8">
        <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">
          <EntryPoint
            icon={<Search />}
            title={user ? "Search the registry" : "Browse the registry"}
            description="Explore the full visible corpus, then narrow by keyword or SBOL type."
            to="/search"
          />
          <EntryPoint
            icon={user ? <FilePlus2 /> : <Dna />}
            title={user ? "Contribute designs" : "Search by sequence"}
            description={
              user
                ? "Import SBOL documents into your workspace, validate them, and prepare them for publication."
                : "Find exact or aligned nucleotide matches across the sequences visible to you."
            }
            to={user ? "/contribute" : "/search?kind=sequence"}
          />
          <EntryPoint
            icon={user ? <FolderKanban /> : <Cable />}
            title={user ? "Open your workspace" : "Connect your tools"}
            description={
              user
                ? "Review your designs, collections, drafts, and recent contribution activity."
                : "Use the sbol CLI, connect an AI agent over MCP, or build on the V2 REST API."
            }
            to={user ? "/workspace" : "/connect"}
          />
          <EntryPoint
            icon={<DatabaseZap />}
            title={
              session.data?.user?.is_admin
                ? "Open admin workspace"
                : user
                  ? "Account and access"
                  : "Manage your designs"
            }
            description={
              session.data?.user?.is_admin
                ? "Inspect data, run queries, and operate this instance from the admin workspace."
                : user
                  ? "Review your profile, membership, and account security settings."
                  : "Sign in to work with private designs and account-scoped data."
            }
            to={
              session.data?.user?.is_admin
                ? "/admin"
                : user
                  ? "/account"
                  : "/login"
            }
          />
        </div>
      </section>

      <MachineAccessSection
        mcpServerAddress={instance.data?.machine_access?.mcp_url}
      />

      <section className="border-y bg-muted/15">
        <div className="mx-auto max-w-7xl px-4 py-14 sm:px-6 lg:px-8">
          <div className="mb-7 flex items-end justify-between gap-4">
            <div>
              <p className="text-xs font-medium uppercase tracking-[0.16em] text-primary">
                Registry
              </p>
              <h2 className="mt-2 text-2xl font-semibold tracking-tight">
                {user ? "Recent designs" : "Explore recent designs"}
              </h2>
              <p className="mt-2 text-sm text-muted-foreground">
                A starting point from the objects visible to your current
                session.
              </p>
            </div>
            <Button asChild variant="outline" size="sm">
              <Link to="/search">
                View all <ArrowRight />
              </Link>
            </Button>
          </div>

          {recent.isLoading ? (
            <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
              {Array.from({ length: 6 }).map((_, index) => (
                <Skeleton key={index} className="h-44 rounded-xl" />
              ))}
            </div>
          ) : recent.data?.items.length ? (
            <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
              {recent.data.items.map((hit) => (
                <ObjectResultCard key={hit.uri} hit={hit} />
              ))}
            </div>
          ) : (
            <div className="rounded-xl border border-dashed bg-background px-6 py-12 text-center">
              <Boxes className="mx-auto size-6 text-muted-foreground/60" />
              <h3 className="mt-3 font-medium">No public designs yet</h3>
              <p className="mt-1 text-sm text-muted-foreground">
                Imported and published objects will appear here.
              </p>
            </div>
          )}
        </div>
      </section>
    </>
  );
}

function EntryPoint({
  icon,
  title,
  description,
  to,
  href,
}: {
  icon: React.ReactNode;
  title: string;
  description: string;
  to?: string;
  href?: string;
}) {
  const content = (
    <>
      <span className="flex size-10 items-center justify-center rounded-xl bg-primary/10 text-primary [&>svg]:size-4">
        {icon}
      </span>
      <div className="mt-5 flex items-center gap-2 font-semibold tracking-tight">
        {title}
        <ArrowRight className="size-4 text-muted-foreground group-hover:text-primary" />
      </div>
      <p className="mt-2 text-sm leading-6 text-muted-foreground">
        {description}
      </p>
    </>
  );
  const className =
    "group rounded-xl border bg-card p-5 shadow-sm transition-[border-color,box-shadow] duration-150 [transition-timing-function:cubic-bezier(0.23,1,0.32,1)] hover:border-primary/35 hover:shadow-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2";
  return href ? (
    <a href={href} className={className}>
      {content}
    </a>
  ) : (
    <Link to={to || "/"} className={className}>
      {content}
    </Link>
  );
}
