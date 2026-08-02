import {
  ArrowRight,
  Boxes,
  Cable,
  DatabaseZap,
  Dna,
  FilePlus2,
  FolderKanban,
  Search,
} from "lucide-react";
import { Link, useNavigate } from "react-router-dom";

import { ObjectResultCard } from "@/components/portal/ObjectResultCard";
import { SearchBox } from "@/components/portal/SearchBox";
import { SbolVisualCassette } from "@/components/portal/SbolVisualCassette";
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
      <section className="registry-field overflow-hidden border-b border-foreground/15">
        <div className="mx-auto grid max-w-[90rem] lg:grid-cols-[1.08fr_0.92fr]">
          <div className="px-4 py-14 sm:px-6 sm:py-20 lg:border-r lg:border-foreground/15 lg:px-8 lg:py-24 xl:pr-16">
            <p className="ledger-label text-primary">
              Open biological design infrastructure
            </p>
            <h1 className="mt-5 max-w-4xl text-balance text-5xl font-medium leading-[0.96] tracking-[-0.045em] sm:text-6xl lg:text-[clamp(3rem,4.5vw,4.5rem)]">
              <span className="block md:whitespace-nowrap">
                Find, share, and reuse
              </span>
              <span className="block">biological designs.</span>
            </h1>
            <p className="mt-7 max-w-2xl text-pretty text-base leading-7 text-muted-foreground sm:text-lg">
              Search {PRODUCT_NAME} for parts, systems, sequences, and
              collections. Every result keeps its SBOL identity, provenance, and
              machine-readable representation close at hand.
            </p>
            <SearchBox
              size="hero"
              className="mt-9 max-w-3xl"
              onSearch={(query) =>
                navigate(
                  query ? `/search?q=${encodeURIComponent(query)}` : "/search"
                )
              }
            />
            <dl className="mt-7 grid max-w-3xl gap-px overflow-hidden border-y border-foreground/15 bg-foreground/15 text-xs sm:grid-cols-3">
              <RegistryFact term="Scope" detail="ACL-aware records" />
              <RegistryFact term="Standard" detail="SBOL 2 and SBOL 3" />
              <RegistryFact term="Access" detail="REST, RDF, and files" />
            </dl>
          </div>
          <RegistryPrimer />
        </div>
      </section>

      {instance.data?.front_page_text && (
        <section className="border-b border-foreground/15 bg-muted/20">
          <div className="mx-auto max-w-[90rem] border-l-2 border-primary px-4 py-7 text-sm leading-7 text-muted-foreground sm:px-6 lg:px-8">
            {instance.data.front_page_text}
          </div>
        </section>
      )}

      <section className="mx-auto max-w-[90rem] px-4 py-14 sm:px-6 lg:px-8 lg:py-18">
        <div className="grid gap-8 lg:grid-cols-[0.36fr_0.64fr]">
          <div>
            <p className="ledger-label text-primary">Registry index</p>
            <h2 className="mt-3 max-w-sm text-3xl font-medium leading-tight tracking-[-0.025em]">
              Choose the view that matches the question.
            </h2>
            <p className="mt-4 max-w-sm text-sm leading-6 text-muted-foreground">
              Search biological meaning, compare sequences, connect tools, or
              manage permissioned records.
            </p>
          </div>
          <div className="border-b border-foreground/15">
            <EntryPoint
              number="01"
              icon={<Search />}
              title={user ? "Search the registry" : "Browse the registry"}
              description="Explore the full visible corpus, then narrow by keyword or SBOL type."
              to="/search"
              tone="promoter"
            />
            <EntryPoint
              number="02"
              icon={user ? <FilePlus2 /> : <Dna />}
              title={user ? "Contribute designs" : "Search by sequence"}
              description={
                user
                  ? "Import SBOL documents into your workspace, validate them, and prepare them for publication."
                  : "Find exact or aligned nucleotide matches across the sequences visible to you."
              }
              to={user ? "/contribute" : "/search?kind=sequence"}
              tone="rbs"
            />
            <EntryPoint
              number="03"
              icon={user ? <FolderKanban /> : <Cable />}
              title={user ? "Open your workspace" : "Connect your tools"}
              description={
                user
                  ? "Review your designs, collections, drafts, and recent contribution activity."
                  : "Use the sbol CLI, connect an AI agent over MCP, or build on the V2 REST API."
              }
              to={user ? "/workspace" : "/connect"}
              tone="cds"
            />
            <EntryPoint
              number="04"
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
              tone="terminator"
            />
          </div>
        </div>
      </section>

      <section className="border-y border-foreground/15 bg-muted/15">
        <div className="mx-auto max-w-[90rem] px-4 py-14 sm:px-6 lg:px-8">
          <div className="mb-7 flex items-end justify-between gap-4">
            <div>
              <p className="ledger-label text-primary">Registry ledger</p>
              <h2 className="mt-2 text-3xl font-medium tracking-tight">
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
            <div className="divide-y divide-foreground/15 border-y border-foreground/15">
              {Array.from({ length: 6 }).map((_, index) => (
                <Skeleton key={index} className="h-28 rounded-none" />
              ))}
            </div>
          ) : recent.data?.items.length ? (
            <div className="divide-y divide-foreground/15 border-y border-foreground/15">
              {recent.data.items.map((hit) => (
                <ObjectResultCard key={hit.uri} hit={hit} variant="row" />
              ))}
            </div>
          ) : (
            <div className="border-y border-dashed border-foreground/25 bg-background/45 px-6 py-12 text-center">
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
  number,
  icon,
  title,
  description,
  to,
  href,
  tone,
}: {
  number: string;
  icon: React.ReactNode;
  title: string;
  description: string;
  to?: string;
  href?: string;
  tone: "promoter" | "cds" | "rbs" | "terminator";
}) {
  const toneClass = {
    promoter: "text-sbol-promoter",
    cds: "text-sbol-cds",
    rbs: "text-sbol-rbs",
    terminator: "text-sbol-terminator",
  }[tone];
  const content = (
    <>
      <span
        className={`flex size-9 items-center justify-center ${toneClass} [&>svg]:size-4`}
      >
        {icon}
      </span>
      <div className="min-w-0">
        <div className="font-medium tracking-tight">{title}</div>
        <p className="mt-1 text-sm leading-6 text-muted-foreground">
          {description}
        </p>
      </div>
      <ArrowRight className="mt-2 size-4 text-muted-foreground transition-transform duration-150 [transition-timing-function:var(--ease-out)] group-hover:translate-x-1 group-hover:text-primary motion-reduce:transition-none" />
    </>
  );
  const className =
    "group grid grid-cols-[2.5rem_minmax(0,1fr)_auto] items-start gap-4 border-t border-foreground/15 px-1 py-5 transition-[background-color,color] duration-150 hover:bg-accent/35 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring";
  return href ? (
    <a href={href} className={className}>
      <span className="sr-only">{number}. </span>
      {content}
    </a>
  ) : (
    <Link to={to || "/"} className={className}>
      <span className="sr-only">{number}. </span>
      {content}
    </Link>
  );
}

function RegistryFact({ term, detail }: { term: string; detail: string }) {
  return (
    <div className="bg-background/90 px-4 py-3">
      <dt className="font-mono text-[9px] uppercase tracking-[0.16em] text-muted-foreground">
        {term}
      </dt>
      <dd className="mt-1 font-medium text-foreground">{detail}</dd>
    </div>
  );
}

function RegistryPrimer() {
  const fields = [
    {
      term: "Identity",
      detail: "One design, unambiguously named and versioned.",
    },
    {
      term: "Composition",
      detail: "Sequences and features arranged into a coherent whole.",
    },
    {
      term: "Function",
      detail: "Roles and interactions describe biological intent.",
    },
    {
      term: "Provenance",
      detail: "Creators, sources, and derivations remain attached.",
    },
  ];
  return (
    <aside className="flex flex-col justify-center bg-card/45 px-4 py-12 sm:px-6 lg:px-10 lg:py-20 xl:px-14">
      <div className="border border-foreground/20 bg-background/85">
        <div className="flex items-center justify-between border-b border-foreground/15 px-4 py-3">
          <span className="ledger-label text-muted-foreground">
            Anatomy of a record
          </span>
          <span className="font-mono text-[10px] text-primary">
            SBOL:COMPONENT
          </span>
        </div>
        <div className="border-b border-foreground/15 bg-muted/15 px-4 py-5 sm:px-5">
          <p className="font-display text-xl font-medium tracking-[-0.02em]">
            What makes a design reusable.
          </p>
          <p className="mt-2 max-w-md text-xs leading-5 text-muted-foreground">
            Reusable designs preserve their identity, composition, function, and
            provenance as they move between people, tools, and repositories.
          </p>
        </div>
        <SbolVisualCassette />
        <dl className="divide-y divide-foreground/15">
          {fields.map(({ term, detail }, index) => (
            <div
              key={term}
              className="grid grid-cols-[2rem_minmax(0,1fr)] gap-x-3 gap-y-1 px-4 py-4 text-xs sm:grid-cols-[2rem_0.66fr_1.34fr] sm:gap-y-0 sm:px-5"
            >
              <span
                aria-hidden="true"
                className="font-mono text-[9px] text-primary"
              >
                {String(index + 1).padStart(2, "0")}
              </span>
              <dt className="font-medium">{term}</dt>
              <dd className="col-start-2 leading-5 text-muted-foreground sm:col-start-3 sm:row-start-1">
                {detail}
              </dd>
            </div>
          ))}
        </dl>
      </div>
    </aside>
  );
}
