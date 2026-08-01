import {
  ArrowRight,
  Bot,
  Check,
  Copy,
  ExternalLink,
  FileCheck2,
  Fingerprint,
  LockKeyhole,
  RefreshCw,
  Search,
  ShieldCheck,
  Terminal,
  Users,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { useCopyToClipboard } from "@/hooks/useCopyToClipboard";

const CLI_REFERENCE =
  "https://github.com/SynBioDex/sbol-rs/tree/master/crates/sbol-cli";
const MCP_SERVER_ADDRESS = "https://sbol.io/mcp";

const steps: CapabilityStepProps[] = [
  {
    number: "01",
    icon: Terminal,
    title: "Design locally",
    description:
      "Validate, compare, convert, and import SBOL 2 and SBOL 3 with the sbol CLI.",
  },
  {
    number: "02",
    icon: RefreshCw,
    title: "Sync with a registry",
    description:
      "Pull canonical designs, inspect changes, and publish through an authenticated SBOL DB profile.",
  },
  {
    number: "03",
    icon: Bot,
    title: "Collaborate through agents",
    description:
      "Give AI tools the same permissioned design context, validation, and review workflows.",
  },
];

const capabilityGroups: CapabilityGroupProps[] = [
  {
    label: "Find and understand designs",
    icon: Search,
    description: "Find the right design and understand its biological context.",
    capabilities: [
      {
        title: "Search the registry",
        description:
          "Find designs by biology, metadata, or sequence similarity.",
      },
      {
        title: "Open complete design records",
        description:
          "Review identity, provenance, collections, and citations together.",
      },
      {
        title: "Download a design",
        description: "Export SBOL 2 or 3 in the format your workflow needs.",
      },
      {
        title: "Find related sequences",
        description: "Compare similar designs with alignment evidence.",
      },
    ],
  },
  {
    label: "Prepare and publish changes",
    icon: FileCheck2,
    description:
      "Prepare changes while every write remains deliberate and reviewable.",
    capabilities: [
      {
        title: "Check a design before upload",
        description:
          "Validate structure and identity collisions without changing the registry.",
      },
      {
        title: "Add a collection",
        description:
          "Create, replace, or merge SBOL, FASTA, and GenBank designs.",
      },
      {
        title: "Improve an existing record",
        description: "Update metadata, notes, provenance, and citations.",
      },
      {
        title: "Publish a stable identity",
        description: "Make a design public under an explicit collision policy.",
      },
    ],
  },
  {
    label: "Share and review together",
    icon: Users,
    description: "Keep sharing and review connected to the design.",
    capabilities: [
      {
        title: "Share without changing ownership",
        description: "Grant or revoke read access to a private design.",
      },
      {
        title: "Start a review",
        description: "Open a review cycle and bring in a curator.",
      },
      {
        title: "Capture the decision",
        description:
          "Record approval or requested changes in the review history.",
      },
      {
        title: "See what happened",
        description: "Trace sharing, ownership, and review activity.",
      },
    ],
  },
];

export function MachineAccessSection() {
  return (
    <section
      id="machine-access"
      className="relative scroll-mt-16 overflow-hidden border-y bg-muted/20"
    >
      <div className="pointer-events-none absolute inset-0 bg-[radial-gradient(circle_at_85%_8%,hsl(var(--primary)/0.12),transparent_28%)]" />
      <div className="relative mx-auto max-w-7xl px-4 py-14 sm:px-6 sm:py-16 lg:px-8">
        <div className="grid gap-8 lg:grid-cols-[1.05fr_0.95fr] lg:items-end">
          <div>
            <p className="text-xs font-medium uppercase tracking-[0.18em] text-primary">
              Machine access
            </p>
            <h2 className="mt-3 max-w-3xl text-balance text-3xl font-semibold tracking-[-0.025em] sm:text-4xl">
              One design language for people, pipelines, and agents.
            </h2>
          </div>
          <div>
            <p className="max-w-2xl text-pretty text-sm leading-7 text-muted-foreground sm:text-base">
              Move from a local SBOL file to a permissioned, collaborative
              record without losing identity or context. The{" "}
              <code className="text-foreground">sbol</code> CLI owns
              standards-aware file workflows; SBOL DB carries registry state,
              access control, and provenance; MCP opens that same contract to
              agents.
            </p>
            <div className="mt-4 flex flex-wrap gap-2">
              <Badge
                variant="outline"
                className="gap-1.5 border-primary/20 bg-background/60"
              >
                <ShieldCheck className="size-3" /> One identity and ACL model
              </Badge>
            </div>
          </div>
        </div>

        <div className="relative mt-10">
          <div
            aria-hidden="true"
            className="absolute left-[16.67%] right-[16.67%] top-7 hidden border-t border-dashed border-primary/25 md:block"
          />
          <ol className="relative grid gap-5 md:grid-cols-3">
            {steps.map((step) => (
              <CapabilityStep key={step.number} {...step} />
            ))}
          </ol>
        </div>

        <div className="mt-6 grid items-start gap-5 lg:grid-cols-[1.1fr_0.9fr]">
          <CliPreview />
          <RegistryPromise />
        </div>

        <McpDocumentation />

        <div className="mt-6 flex flex-col gap-3 border-t pt-5 text-sm sm:flex-row sm:items-center sm:justify-between">
          <p className="max-w-2xl text-muted-foreground">
            The boundary stays explicit: the CLI evolves with the SBOL SDK; SBOL
            DB owns authenticated registry and agent access.
          </p>
          <div className="flex flex-wrap items-center gap-x-5 gap-y-2">
            <a
              href={CLI_REFERENCE}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1.5 font-medium text-foreground underline-offset-4 hover:text-primary hover:underline focus-visible:rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              Explore the sbol CLI <ExternalLink className="size-3.5" />
            </a>
            <a
              href="/api/v2/docs"
              className="inline-flex items-center gap-1.5 font-medium text-foreground underline-offset-4 hover:text-primary hover:underline focus-visible:rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              Inspect the REST contract <ArrowRight className="size-3.5" />
            </a>
          </div>
        </div>
      </div>
    </section>
  );
}

type CapabilityStepProps = {
  number: string;
  icon: LucideIcon;
  title: string;
  description: string;
};

function CapabilityStep({
  number,
  icon: Icon,
  title,
  description,
}: CapabilityStepProps) {
  return (
    <li className="relative rounded-2xl border bg-card/90 p-4 shadow-sm backdrop-blur sm:p-5">
      <div className="flex items-center justify-between gap-3">
        <span className="relative z-10 flex size-12 items-center justify-center rounded-xl border bg-background text-primary shadow-sm">
          <Icon className="size-5" />
        </span>
        <span className="font-mono text-[10px] tracking-[0.18em] text-muted-foreground">
          {number}
        </span>
      </div>
      <h3 className="mt-4 font-semibold tracking-tight">{title}</h3>
      <p className="mt-2 text-xs leading-5 text-muted-foreground sm:text-sm sm:leading-6">
        {description}
      </p>
    </li>
  );
}

function CliPreview() {
  return (
    <div className="self-start overflow-hidden rounded-2xl border border-white/10 bg-zinc-950 text-zinc-100 shadow-xl shadow-primary/5">
      <div className="flex items-center gap-2 border-b border-white/10 px-5 py-3.5">
        <span className="size-2.5 rounded-full bg-red-400/80" />
        <span className="size-2.5 rounded-full bg-amber-300/80" />
        <span className="size-2.5 rounded-full bg-emerald-400/80" />
      </div>
      <div className="space-y-5 px-5 py-5 font-mono text-xs leading-6 sm:px-6 sm:text-[13px]">
        <div>
          <div className="mb-2 font-sans text-[11px] font-medium uppercase tracking-[0.14em] text-emerald-300">
            Build confidence locally
          </div>
          <Command>sbol validate toggle-switch.ttl</Command>
          <Command>sbol diff baseline.ttl candidate.ttl</Command>
        </div>
        <div className="border-t border-white/10 pt-4">
          <div className="mb-2 font-sans text-[11px] font-medium uppercase tracking-[0.14em] text-emerald-300">
            Carry the same design into the registry
          </div>
          <Command>sbol registry login</Command>
          <Command>
            {
              "sbol registry pull https://sbol.io/public/igem/BBa_J23100/1 -o design.ttl"
            }
          </Command>
          <Command>sbol registry sync design.ttl --preview</Command>
          <Command>sbol registry push design.ttl</Command>
        </div>
      </div>
    </div>
  );
}

function RegistryPromise() {
  const promises = [
    {
      icon: Fingerprint,
      title: "Identity survives the handoff",
      description:
        "Canonical IRIs, SBOL versions, and serialization choices remain explicit from disk to registry.",
    },
    {
      icon: LockKeyhole,
      title: "Private means permissioned",
      description:
        "The same caller scope governs the UI, REST API, CLI, and every agent request.",
    },
    {
      icon: FileCheck2,
      title: "Changes are inspectable",
      description:
        "Validation, collision analysis, ownership, review, and activity evidence travel with the workflow.",
    },
  ];

  return (
    <div className="rounded-2xl border bg-card p-5 shadow-sm sm:p-6">
      <p className="text-xs font-medium uppercase tracking-[0.16em] text-primary">
        SBOL DB CLI
      </p>
      <h3 className="mt-2 text-xl font-semibold tracking-tight">
        Keep the biological and social context together.
      </h3>
      <div className="mt-5 space-y-4">
        {promises.map(({ icon: Icon, title, description }) => (
          <div key={title} className="flex gap-3">
            <span className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
              <Icon className="size-4" />
            </span>
            <div>
              <h4 className="text-sm font-medium">{title}</h4>
              <p className="mt-1 text-xs leading-5 text-muted-foreground">
                {description}
              </p>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

function Command({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex min-w-0 gap-3">
      <span aria-hidden="true" className="select-none text-primary">
        $
      </span>
      <code className="min-w-0 whitespace-pre-wrap break-words text-zinc-200 sm:whitespace-nowrap">
        {children}
      </code>
    </div>
  );
}

function McpDocumentation() {
  const clipboard = useCopyToClipboard();
  const copyLabel = clipboard.copied
    ? "Server address copied"
    : clipboard.failed
      ? "Could not copy server address"
      : "Copy server address";

  return (
    <div
      id="mcp"
      className="mt-8 scroll-mt-24 overflow-hidden rounded-3xl border bg-card shadow-sm"
    >
      <div className="grid lg:grid-cols-[1.35fr_0.65fr]">
        <div className="p-6 sm:p-8">
          <div className="flex items-start gap-3.5">
            <span className="mt-0.5 flex size-10 shrink-0 items-center justify-center rounded-xl border border-primary/10 bg-primary/10 text-primary shadow-sm">
              <Bot className="size-[18px]" />
            </span>
            <div className="max-w-2xl">
              <p className="text-[11px] font-semibold uppercase tracking-[0.18em] text-primary">
                SBOL DB MCP
              </p>
              <h3 className="mt-1.5 text-balance text-2xl font-semibold tracking-[-0.025em]">
                Let your agent work safely with biological designs.
              </h3>
              <p className="mt-2 max-w-xl text-sm leading-6 text-muted-foreground">
                Find designs, prepare changes, and move reviews forward—without
                stepping outside your SBOL DB permissions.
              </p>
            </div>
          </div>

          <div className="mt-7 grid gap-5 sm:grid-cols-3">
            {capabilityGroups.map((group) => (
              <CapabilityGroup key={group.label} {...group} />
            ))}
          </div>
        </div>

        <div className="flex flex-col border-t bg-zinc-950 p-6 text-zinc-100 lg:border-l lg:border-t-0 sm:p-8">
          <div className="flex items-center justify-between gap-4">
            <div>
              <p className="text-[10px] font-semibold uppercase tracking-[0.18em] text-zinc-500">
                Connect your agent
              </p>
              <h4 className="mt-2 text-lg font-semibold tracking-[-0.015em]">
                Add SBOL DB in a few clicks.
              </h4>
            </div>
          </div>

          <div className="mt-6 rounded-xl border border-white/10 bg-white/[0.04] p-4">
            <p className="text-[10px] font-medium uppercase tracking-[0.14em] text-zinc-500">
              Server address
            </p>
            <div className="mt-2 flex items-center gap-3">
              <code className="min-w-0 flex-1 break-all text-[13px] font-medium text-zinc-100">
                {MCP_SERVER_ADDRESS}
              </code>
              <button
                type="button"
                onClick={() => clipboard.copy(MCP_SERVER_ADDRESS)}
                aria-label={copyLabel}
                title={copyLabel}
                className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-sky-300 text-zinc-950 shadow-sm shadow-sky-500/20 transition-colors hover:bg-sky-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-300 focus-visible:ring-offset-2 focus-visible:ring-offset-zinc-950"
              >
                {clipboard.copied ? (
                  <Check className="size-4" />
                ) : (
                  <Copy className="size-4" />
                )}
              </button>
            </div>
            <span className="sr-only" aria-live="polite">
              {clipboard.status === "idle" ? "" : copyLabel}
            </span>
          </div>

          <ol className="mt-6 space-y-5">
            <ConnectionStep
              number="1"
              title="Add the server address"
              description="Paste it into the connections or MCP settings in your AI agent."
            />
            <ConnectionStep
              number="2"
              title="Sign in to SBOL DB"
              description="Use your normal registry account when your agent prompts you."
            />
            <ConnectionStep
              number="3"
              title="Start with a request"
              description="Ask your agent to find, validate, share, or review a design."
            />
          </ol>

          <div className="mt-auto pt-6">
            <div className="rounded-xl border border-sky-300/10 bg-sky-300/[0.04] p-4">
              <div className="flex gap-3">
                <LockKeyhole className="mt-0.5 size-4 shrink-0 text-sky-300" />
                <div>
                  <p className="text-xs font-medium text-zinc-100">
                    Your access rules still apply.
                  </p>
                  <p className="mt-1 text-xs leading-5 text-zinc-400">
                    Your agent can only see and change what you can. Public,
                    shared, and private designs continue to follow your SBOL DB
                    permissions.
                  </p>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

function ConnectionStep({
  number,
  title,
  description,
}: {
  number: string;
  title: string;
  description: string;
}) {
  return (
    <li className="flex gap-3">
      <span className="flex size-6 shrink-0 items-center justify-center rounded-full border border-white/10 bg-white/5 font-mono text-[10px] text-sky-300">
        {number}
      </span>
      <div>
        <p className="text-xs font-medium text-zinc-100">{title}</p>
        <p className="mt-1 text-xs leading-5 text-zinc-400">{description}</p>
      </div>
    </li>
  );
}

type CapabilityGroupProps = {
  label: string;
  icon: LucideIcon;
  description: string;
  capabilities: Array<{ title: string; description: string }>;
};

function CapabilityGroup({
  label,
  icon: Icon,
  description,
  capabilities,
}: CapabilityGroupProps) {
  return (
    <article className="border-t pt-5 first:border-t-0 first:pt-0 sm:border-l sm:border-t-0 sm:pl-5 sm:pt-0 sm:first:border-l-0 sm:first:pl-0">
      <span className="flex size-8 items-center justify-center rounded-lg border border-primary/10 bg-primary/10 text-primary">
        <Icon className="size-[15px]" />
      </span>
      <h4 className="mt-3 text-sm font-semibold tracking-[-0.015em]">
        {label}
      </h4>
      <p className="mt-1.5 text-xs leading-[1.55] text-muted-foreground">
        {description}
      </p>

      <ul className="mt-4 divide-y" role="list">
        {capabilities.map((capability) => (
          <li key={capability.title} className="py-2.5 first:pt-0 last:pb-0">
            <h5 className="text-xs font-semibold text-foreground">
              {capability.title}
            </h5>
            <p className="mt-1 text-xs leading-[1.45] text-muted-foreground">
              {capability.description}
            </p>
          </li>
        ))}
      </ul>
    </article>
  );
}
