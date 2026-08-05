import { ArrowRight, Cable, Dna, FileCode2, Search, Users } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { Link } from "react-router-dom";

import { Button } from "@/components/ui/button";
import { API_DOCS_PATH } from "@/lib/routes";

const waysToWork: Array<{
  icon: LucideIcon;
  eyebrow: string;
  title: string;
  description: string;
}> = [
  {
    icon: Search,
    eyebrow: "Registry",
    title: "Explore and evaluate designs",
    description:
      "Browse complete records, inspect biological relationships, compare sequences, and download the representation your workflow needs.",
  },
  {
    icon: Users,
    eyebrow: "Workspace",
    title: "Contribute and collaborate",
    description:
      "Validate imports before they write, review minted identities, organize collections, and share designs with people or teams.",
  },
  {
    icon: Cable,
    eyebrow: "API access",
    title: "Build on the registry",
    description:
      "Use the REST API through the same identity and permission boundaries as the browser application.",
  },
];

const applicationAreas: Array<{
  number: string;
  eyebrow: string;
  title: string;
  description: string;
  capabilities: string[];
  to: string;
  action: string;
  external?: boolean;
}> = [
  {
    number: "01",
    eyebrow: "Public registry",
    title: "Discover and inspect biological designs",
    description:
      "The public application turns registry records into browsable, connected biological context without hiding their machine identities.",
    capabilities: [
      "Search names, identifiers, types, roles, and biological meaning",
      "Search DNA by exact, reverse-complement, alignment, or configured similarity methods",
      "Inspect composition, sequences, provenance, collections, citations, and related records",
      "Download available SBOL and sequence representations",
    ],
    to: "/search",
    action: "Open registry search",
  },
  {
    number: "02",
    eyebrow: "Account workspace",
    title: "Contribute, organize, share, and review",
    description:
      "Signed-in workflows make persistence consequences and collaboration boundaries visible before they change a registry.",
    capabilities: [
      "Validate SBOL, GenBank, and FASTA before committing an import",
      "Review minted identities, conversions, collisions, and collection membership",
      "Separate owned designs, shared records, and review requests",
      "Manage profile, password, publication, sharing, and ownership workflows",
    ],
    to: "/contribute",
    action: "Contribute a design",
  },
  {
    number: "03",
    eyebrow: "API access",
    title: "Integrate applications and scripts",
    description:
      "The browser is one client of the registry. The same data and authorization model is available through documented REST interfaces.",
    capabilities: [
      "Switch among the SBOL DB API, the SynBioHub v1 compatibility API, and the SynBioHub v2 API",
      "Read normalized object, collection, and sequence representations",
      "Build integrations within the registry's authorization boundaries",
      "Automate documented searches, queries, and downloads",
    ],
    to: API_DOCS_PATH,
    action: "Open API reference",
    external: true,
  },
  {
    number: "04",
    eyebrow: "Administrator workspace",
    title: "Operate the registry as a system",
    description:
      "The administrator application exposes data, query, maintenance, and governance surfaces without weakening their server-side controls.",
    capabilities: [
      "Inspect graphs, objects, sequences, ontologies, and backend schema",
      "Query RDF with SPARQL and supported relational backends with SQL",
      "Monitor metrics, jobs, storage maintenance, and search indexes",
      "Manage instance policy, users, integrations, backups, and audit activity",
    ],
    to: "/admin",
    action: "Open administrator workspace",
  },
];

export default function AboutRoute() {
  return (
    <div>
      <section className="registry-field border-b border-foreground/15">
        <div className="mx-auto grid max-w-[90rem] lg:grid-cols-[1.1fr_0.9fr]">
          <div className="px-4 py-14 sm:px-6 sm:py-20 lg:border-r lg:border-foreground/15 lg:px-8 lg:py-24 xl:pr-16">
            <p className="ledger-label text-primary">About SBOL DB</p>
            <h1 className="mt-5 max-w-4xl text-balance text-4xl font-medium leading-[1.02] tracking-[-0.04em] sm:text-5xl lg:text-6xl">
              Infrastructure for sharing biological designs.
            </h1>
            <p className="mt-7 max-w-2xl text-pretty text-base leading-7 text-muted-foreground sm:text-lg">
              SBOL DB is an open registry and query system for standardized
              synthetic biology designs. It keeps identity, biological
              structure, sequences, provenance, permissions, and machine
              representations together as designs move between people and tools.
            </p>
            <div className="mt-8 flex flex-wrap gap-3">
              <Button asChild>
                <a href={API_DOCS_PATH}>
                  API reference <ArrowRight />
                </a>
              </Button>
              <Button asChild variant="outline">
                <Link to="/search">Search designs</Link>
              </Button>
            </div>
          </div>

          <div className="flex items-center bg-background px-4 py-12 sm:px-6 lg:px-10 lg:py-20 xl:px-14">
            <dl className="w-full border border-foreground/15 bg-background">
              <AboutFact
                term="Formats"
                detail="SBOL2, SBOL3, GenBank, and FASTA"
              />
              <AboutFact
                term="Discovery"
                detail="Identity, metadata, meaning, graph, and sequence"
              />
              <AboutFact term="Interfaces" detail="Browser and REST API" />
              <AboutFact term="Storage" detail="RocksDB, Postres, SQLite" />
            </dl>
          </div>
        </div>
      </section>

      <nav
        aria-label="About page sections"
        className="border-b border-foreground/15 bg-card"
      >
        <div className="mx-auto flex max-w-[90rem] gap-6 overflow-x-auto px-4 py-3 font-mono text-[10px] uppercase tracking-[0.12em] text-muted-foreground sm:px-6 lg:px-8">
          <a className="shrink-0 hover:text-foreground" href="#purpose">
            Purpose
          </a>
          <a className="shrink-0 hover:text-foreground" href="#ways-to-work">
            Ways to work
          </a>
          <a className="shrink-0 hover:text-foreground" href="#application-map">
            Application map
          </a>
          <a className="shrink-0 hover:text-foreground" href="#scope">
            Scope
          </a>
        </div>
      </nav>

      <section
        id="purpose"
        className="mx-auto scroll-mt-24 max-w-[90rem] px-4 py-14 sm:px-6 lg:px-8 lg:py-20"
      >
        <div className="grid gap-10 lg:grid-cols-[0.38fr_0.62fr] lg:gap-16">
          <div>
            <p className="ledger-label text-primary">Why it exists</p>
            <h2 className="mt-3 text-3xl font-medium tracking-[-0.025em]">
              A design is more than a file.
            </h2>
          </div>
          <div className="max-w-3xl space-y-5 text-base leading-7 text-muted-foreground">
            <p>
              A biological design carries names, sequences, features, roles,
              interactions, provenance, and relationships to other designs. When
              that structure is reduced to an attachment or copied between
              disconnected systems, the context needed to evaluate and reuse it
              is easily lost.
            </p>
            <p>
              SBOL DB treats the design identity and its structured record as
              the durable center of the workflow. The browser, API, and
              connected applications all work from the same underlying data and
              permission model.
            </p>
          </div>
        </div>
      </section>

      <section
        id="ways-to-work"
        className="scroll-mt-24 border-y border-foreground/15 bg-muted/15"
      >
        <div className="mx-auto max-w-[90rem] px-4 py-14 sm:px-6 lg:px-8 lg:py-20">
          <div className="max-w-3xl">
            <p className="ledger-label text-primary">One registry</p>
            <h2 className="mt-3 text-3xl font-medium tracking-[-0.025em]">
              Several ways to work with the same designs.
            </h2>
            <p className="mt-4 text-base leading-7 text-muted-foreground">
              Each surface is designed for a different task, but none creates a
              separate copy of the domain or its access rules.
            </p>
          </div>

          <div className="mt-10 grid gap-px border bg-foreground/15 lg:grid-cols-3">
            {waysToWork.map((way) => (
              <article key={way.eyebrow} className="bg-background p-6 sm:p-8">
                <div className="flex items-center gap-3 text-primary">
                  <way.icon className="size-5" aria-hidden="true" />
                  <p className="ledger-label">{way.eyebrow}</p>
                </div>
                <h3 className="mt-5 text-xl font-semibold tracking-tight">
                  {way.title}
                </h3>
                <p className="mt-3 text-sm leading-6 text-muted-foreground">
                  {way.description}
                </p>
              </article>
            ))}
          </div>
        </div>
      </section>

      <section
        id="application-map"
        className="mx-auto scroll-mt-24 max-w-[90rem] px-4 py-14 sm:px-6 lg:px-8 lg:py-20"
      >
        <div className="grid gap-10 lg:grid-cols-[0.34fr_0.66fr] lg:gap-16">
          <div>
            <p className="ledger-label text-primary">Application map</p>
            <h2 className="mt-3 text-3xl font-medium tracking-[-0.025em]">
              The complete SBOL DB application.
            </h2>
            <p className="mt-4 text-sm leading-6 text-muted-foreground">
              The product is organized by responsibility. Public discovery,
              account collaboration, machine integration, and system operation
              share one domain without sharing every privilege.
            </p>
          </div>

          <div className="border-t border-foreground/15">
            {applicationAreas.map((area) => (
              <article
                key={area.number}
                className="grid gap-5 border-b border-foreground/15 py-7 sm:grid-cols-[3rem_minmax(0,1fr)]"
              >
                <span className="font-mono text-[10px] text-primary">
                  {area.number}
                </span>
                <div>
                  <p className="ledger-label text-primary">{area.eyebrow}</p>
                  <h3 className="mt-2 text-xl font-semibold tracking-tight">
                    {area.title}
                  </h3>
                  <p className="mt-3 max-w-2xl text-sm leading-6 text-muted-foreground">
                    {area.description}
                  </p>
                  <ul className="mt-5 grid gap-2 text-sm leading-6 text-muted-foreground sm:grid-cols-2 sm:gap-x-8">
                    {area.capabilities.map((capability) => (
                      <li key={capability} className="flex gap-2.5">
                        <span
                          className="mt-[0.65rem] size-1 shrink-0 bg-primary"
                          aria-hidden="true"
                        />
                        <span>{capability}</span>
                      </li>
                    ))}
                  </ul>
                  <Button asChild variant="link" className="mt-5 h-auto p-0">
                    {area.external ? (
                      <a href={area.to}>
                        {area.action} <ArrowRight />
                      </a>
                    ) : (
                      <Link to={area.to}>
                        {area.action} <ArrowRight />
                      </Link>
                    )}
                  </Button>
                </div>
              </article>
            ))}
          </div>
        </div>
      </section>

      <section
        id="scope"
        className="mx-auto grid scroll-mt-24 max-w-[90rem] gap-10 border-t border-foreground/15 px-4 py-14 sm:px-6 lg:grid-cols-[0.62fr_0.38fr] lg:px-8 lg:py-20"
      >
        <div className="max-w-3xl">
          <div className="flex items-center gap-3 text-primary">
            <Dna className="size-5" aria-hidden="true" />
            <p className="ledger-label">Scope</p>
          </div>
          <h2 className="mt-4 text-3xl font-medium tracking-[-0.025em]">
            Focused on biological designs.
          </h2>
          <p className="mt-4 text-base leading-7 text-muted-foreground">
            SBOL DB is deliberately centered on storing, finding, exchanging,
            and governing SBOL designs. It is not a laboratory orchestration
            system, experiment tracker, predictive-model registry, or complete
            design-build-test-learn platform.
          </p>
        </div>
        <div className="border-l-2 border-primary bg-muted/20 p-6">
          <FileCode2 className="size-5 text-primary" aria-hidden="true" />
          <h3 className="mt-4 font-semibold">Built on open standards</h3>
          <p className="mt-2 text-sm leading-6 text-muted-foreground">
            SBOL3 is the canonical design model. SBOL2 is upgraded on import,
            while GenBank and FASTA inputs are converted into structured SBOL3
            records before they enter the registry.
          </p>
          <Button asChild variant="link" className="mt-3 h-auto p-0">
            <a
              href="https://sbolstandard.org"
              target="_blank"
              rel="noopener noreferrer"
            >
              Learn about the SBOL standard <ArrowRight />
            </a>
          </Button>
        </div>
      </section>
    </div>
  );
}

function AboutFact({ term, detail }: { term: string; detail: string }) {
  return (
    <div className="grid gap-1 border-b border-foreground/15 px-5 py-4 last:border-b-0 sm:grid-cols-[7rem_1fr] sm:gap-4">
      <dt className="font-mono text-[10px] uppercase tracking-[0.14em] text-primary">
        {term}
      </dt>
      <dd className="text-sm font-medium leading-5">{detail}</dd>
    </div>
  );
}
