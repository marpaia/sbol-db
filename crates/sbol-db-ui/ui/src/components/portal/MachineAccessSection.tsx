import {
  ArrowRight,
  Cable,
  Check,
  ClipboardCheck,
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
  Workflow,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { CSSProperties, ReactNode } from "react";

import { Badge } from "@/components/ui/badge";
import { useCopyToClipboard } from "@/hooks/useCopyToClipboard";

const CLI_REFERENCE =
  "https://github.com/SynBioDex/sbol-rs/tree/master/crates/sbol-cli";
const QUIET_ICON_TILE =
  "flex shrink-0 items-center justify-center rounded-[3px] border border-foreground/10 bg-background text-primary";

function currentMcpServerAddress() {
  if (typeof window === "undefined") return "/mcp";
  return new URL("/mcp", window.location.origin).toString();
}

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
      "Check out collections, inspect local and remote changes, and synchronize through your SBOL account.",
  },
  {
    number: "03",
    icon: Workflow,
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

export function MachineAccessSection({
  mcpServerAddress,
}: {
  mcpServerAddress?: string;
}) {
  const serverAddress = mcpServerAddress ?? currentMcpServerAddress();

  return (
    <section
      id="machine-access"
      className="registry-field relative scroll-mt-16 overflow-hidden border-y border-foreground/15 bg-muted/20"
    >
      <div className="relative mx-auto max-w-[90rem] px-4 py-14 sm:px-6 sm:py-16 lg:px-8">
        <div className="grid gap-8 lg:grid-cols-[1.05fr_0.95fr] lg:items-end">
          <div>
            <p className="ledger-label text-primary">
              Identity and machine access
            </p>
            <h1 className="mt-3 max-w-3xl text-balance text-4xl font-medium tracking-[-0.03em] sm:text-5xl">
              Connect your identity, designs, tools, and agents.
            </h1>
          </div>
          <div>
            <p className="max-w-2xl text-pretty text-sm leading-7 text-muted-foreground sm:text-base">
              Sign in to synthetic biology applications with your SBOL Identity.
              Then decide which tools and agents may access or act on your
              designs. The CLI and MCP use the same account, delegated scopes,
              and record-level permissions as the registry.
            </p>
            <div className="mt-4 flex flex-wrap gap-2">
              <Badge
                variant="outline"
                className="gap-1.5 border-primary/20 bg-background/60"
              >
                <Users className="size-3" /> Sign in with SBOL
              </Badge>
              <Badge
                variant="outline"
                className="gap-1.5 border-primary/20 bg-background/60"
              >
                <ShieldCheck className="size-3" /> Scoped agent authority
              </Badge>
              <Badge
                variant="outline"
                className="gap-1.5 border-primary/20 bg-background/60"
              >
                <ClipboardCheck className="size-3" /> Reviewable writes
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

        <IdentityDocumentation />

        <McpDocumentation serverAddress={serverAddress} />

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
    <li className="relative border border-foreground/15 border-t-2 border-t-primary/60 bg-card/90 p-4 sm:p-5">
      <div className="flex items-center justify-between gap-3">
        <span className="relative z-10 flex size-12 items-center justify-center rounded-[3px] border bg-background text-primary">
          <Icon className="size-5" />
        </span>
        <span className="font-mono text-[10px] tracking-[0.18em] text-muted-foreground">
          {number}
        </span>
      </div>
      <h2 className="mt-4 font-semibold tracking-tight">{title}</h2>
      <p className="mt-2 text-xs leading-5 text-muted-foreground sm:text-sm sm:leading-6">
        {description}
      </p>
    </li>
  );
}

function CliPreview() {
  const terminalRef = useRef<HTMLDivElement>(null);
  const [running, setRunning] = useState(false);
  const [session, setSession] = useState(0);

  useEffect(() => {
    const terminal = terminalRef.current;
    if (!terminal) return;
    if (!("IntersectionObserver" in window)) return;

    const observer = new window.IntersectionObserver(
      ([entry]) => {
        if (!entry?.isIntersecting) return;
        setRunning(true);
        observer.disconnect();
      },
      { threshold: 0.35 }
    );
    observer.observe(terminal);
    return () => observer.disconnect();
  }, []);

  return (
    <div
      ref={terminalRef}
      className="self-start overflow-hidden rounded-[4px] border border-white/10 bg-zinc-950 text-zinc-100 shadow-xl shadow-primary/5"
    >
      <div className="flex items-center justify-between gap-4 border-b border-white/10 px-5 py-3.5">
        <div className="flex items-center gap-2" aria-hidden="true">
          <span className="size-2.5 rounded-full bg-red-400/80" />
          <span className="size-2.5 rounded-full bg-amber-300/80" />
          <span className="size-2.5 rounded-full bg-emerald-400/80" />
        </div>
        <button
          type="button"
          onClick={() => {
            setRunning(true);
            setSession((value) => value + 1);
          }}
          className="inline-flex items-center gap-1.5 rounded-[3px] px-2 py-1 font-sans text-[10px] font-medium uppercase tracking-[0.12em] text-zinc-500 transition-[color,background-color,transform] duration-150 [transition-timing-function:var(--ease-out)] hover:bg-white/5 hover:text-zinc-300 active:scale-[0.97] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-300 motion-reduce:transition-none"
        >
          <RefreshCw className="size-3" aria-hidden="true" />
          Replay
        </button>
      </div>
      <TerminalSession key={session} running={running} />
    </div>
  );
}

function TerminalSession({ running }: { running: boolean }) {
  return (
    <div
      className="terminal-session min-h-[27rem] space-y-3 px-5 py-5 font-mono text-xs leading-5 sm:px-6 sm:text-[13px]"
      data-running={running}
      aria-label="Animated SBOL registry command demonstration"
    >
      <AnimatedCommand command="sbol init" delay={200} duration={420}>
        <TerminalSuccess>Created sbol.toml and designs/</TerminalSuccess>
      </AnimatedCommand>

      <AnimatedCommand
        command="sbol pull https://sbol.io/public/igem/BBa_J23100/1"
        delay={1150}
        duration={1450}
      >
        <TerminalProgress
          label="Resolve graph"
          detail="14 objects"
          offset={100}
          duration={360}
        />
        <TerminalProgress
          label="Fetch content"
          detail="42.8 kB"
          offset={520}
          duration={1400}
        />
        <TerminalProgress
          label="Verify content"
          detail="sha256 verified"
          offset={1980}
          duration={520}
        />
        <TerminalProgress
          label="Write lock"
          detail="sbol.lock updated"
          offset={2560}
          duration={280}
        />
      </AnimatedCommand>

      <AnimatedCommand command="sbol status" delay={5750} duration={420}>
        <TerminalMuted>
          BBa_J23100&nbsp;&nbsp;clean&nbsp;&nbsp;local = registry
        </TerminalMuted>
      </AnimatedCommand>

      <AnimatedCommand
        command="sbol sync --dry-run"
        delay={6750}
        duration={620}
      >
        <TerminalMuted>Plan&nbsp;&nbsp;0 pull&nbsp;&nbsp;0 push</TerminalMuted>
        <TerminalSuccess>No registry changes required</TerminalSuccess>
      </AnimatedCommand>

      <AnimatedCommand command="sbol sync" delay={8100} duration={360}>
        <TerminalProgress
          label="Sync workspace"
          detail="synchronized"
          offset={80}
          duration={900}
        />
      </AnimatedCommand>
    </div>
  );
}

type TerminalTimingStyle = CSSProperties & {
  "--terminal-delay": string;
  "--terminal-duration": string;
  "--terminal-characters": number;
};

function AnimatedCommand({
  command,
  delay,
  duration,
  children,
}: {
  command: string;
  delay: number;
  duration: number;
  children: ReactNode;
}) {
  const style: TerminalTimingStyle = {
    "--terminal-delay": `${delay}ms`,
    "--terminal-duration": `${duration}ms`,
    "--terminal-characters": command.length,
  };

  return (
    <div className="terminal-entry" style={style}>
      <div className="flex min-w-0 gap-3">
        <span aria-hidden="true" className="select-none text-emerald-300">
          $
        </span>
        <code className="terminal-command-text min-w-0 max-w-full whitespace-nowrap text-zinc-100">
          {command}
        </code>
      </div>
      <div className="terminal-output ml-6 mt-1 space-y-1.5">{children}</div>
    </div>
  );
}

function TerminalSuccess({ children }: { children: ReactNode }) {
  return (
    <div className="flex items-center gap-2 text-emerald-300">
      <Check className="size-3.5 shrink-0" aria-hidden="true" />
      <span>{children}</span>
    </div>
  );
}

function TerminalMuted({ children }: { children: ReactNode }) {
  return <div className="text-zinc-500">{children}</div>;
}

type ProgressTimingStyle = CSSProperties & {
  "--terminal-progress-offset": string;
  "--terminal-progress-duration": string;
};

function TerminalProgress({
  label,
  detail,
  offset,
  duration,
}: {
  label: string;
  detail: string;
  offset: number;
  duration: number;
}) {
  const style: ProgressTimingStyle = {
    "--terminal-progress-offset": `${offset}ms`,
    "--terminal-progress-duration": `${duration}ms`,
  };

  return (
    <div
      className="terminal-progress grid grid-cols-[6.5rem_minmax(3rem,1fr)_6.5rem] items-center gap-2 sm:grid-cols-[7.5rem_minmax(3rem,1fr)_6.5rem]"
      style={style}
    >
      <span className="truncate text-zinc-400">{label}</span>
      <span
        className="h-1 overflow-hidden rounded-full bg-white/10"
        aria-hidden="true"
      >
        <span className="terminal-progress-fill block h-full origin-left rounded-full bg-emerald-300" />
      </span>
      <span className="terminal-progress-detail whitespace-nowrap text-right text-[10px] text-zinc-500">
        {detail}
      </span>
    </div>
  );
}

function RegistryPromise() {
  const promises = [
    {
      icon: Fingerprint,
      title: "Identity survives the handoff",
      description:
        "Canonical IRIs, SBOL versions, and biological-content fingerprints remain explicit from disk to registry.",
    },
    {
      icon: LockKeyhole,
      title: "Private means permissioned",
      description:
        "The same caller scope governs the UI, REST API, CLI, and every request from your agent.",
    },
    {
      icon: FileCheck2,
      title: "Changes are inspectable",
      description:
        "Validation runs before writes, identity collisions fail by default, and stale synchronized updates are rejected.",
    },
  ];

  return (
    <div className="border border-foreground/15 border-l-2 border-l-primary bg-card p-5 sm:p-6">
      <p className="ledger-label text-primary">SBOL DB CLI</p>
      <h2 className="mt-2 text-xl font-semibold tracking-tight">
        Local when you need it. Connected when the design is shared.
      </h2>
      <p className="mt-3 text-sm leading-6 text-muted-foreground">
        The CLI validates and compares files without a network request. A
        credential-free project file and lock then track complete collections
        across local and registry state without inferring deletion or silently
        merging RDF.
      </p>
      <div className="mt-5 space-y-4">
        {promises.map(({ icon: Icon, title, description }) => (
          <div key={title} className="flex gap-3">
            <span className={`${QUIET_ICON_TILE} mt-0.5 size-8`}>
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

function IdentityDocumentation() {
  const identityBenefits = [
    {
      icon: Users,
      title: "One account",
      description: "Carry your registry identity into compatible applications.",
    },
    {
      icon: Fingerprint,
      title: "Clear consent",
      description: "See which identity details an application is requesting.",
    },
    {
      icon: ShieldCheck,
      title: "Scoped access",
      description: "Tools and agents receive only approved capabilities.",
    },
  ];

  return (
    <section
      id="identity"
      className="mt-8 scroll-mt-24 overflow-hidden rounded-[4px] border border-foreground/15 bg-card"
    >
      <div className="grid lg:grid-cols-[1.35fr_0.65fr]">
        <div className="p-6 sm:p-8">
          <div className="flex items-start gap-3.5">
            <span className={`${QUIET_ICON_TILE} mt-0.5 size-10`}>
              <Fingerprint className="size-[18px]" aria-hidden="true" />
            </span>
            <div className="max-w-2xl">
              <p className="ledger-label text-primary">SBOL Identity</p>
              <h2 className="mt-1.5 text-balance text-2xl font-semibold tracking-[-0.025em]">
                Sign in with your SBOL Identity.
              </h2>
              <p className="mt-2 max-w-xl text-sm leading-6 text-muted-foreground">
                Use one registry account across compatible synthetic biology
                applications, then choose what each application or agent may
                access.
              </p>
            </div>
          </div>

          <div className="mt-7 grid gap-5 sm:grid-cols-3">
            {identityBenefits.map(({ icon: Icon, title, description }) => (
              <div key={title}>
                <Icon className="size-4 text-primary" aria-hidden="true" />
                <h3 className="mt-3 text-sm font-semibold">{title}</h3>
                <p className="mt-1 text-xs leading-5 text-muted-foreground">
                  {description}
                </p>
              </div>
            ))}
          </div>
        </div>

        <div className="flex flex-col border-t bg-zinc-950 p-6 text-zinc-100 lg:border-l lg:border-t-0 sm:p-8">
          <p className="text-[10px] font-semibold uppercase tracking-[0.18em] text-zinc-500">
            Example application
          </p>
          <div
            className="mt-5 rounded-xl border border-white/10 bg-white/[0.04] p-5"
            role="img"
            aria-label="Example application sign-in screen using SBOL Identity"
          >
            <div className="flex items-center gap-3">
              <span className="flex size-9 items-center justify-center rounded-[3px] border border-white/10 bg-white/5 text-sky-300">
                <Fingerprint className="size-4" aria-hidden="true" />
              </span>
              <div>
                <p className="text-sm font-semibold">SynBioSuite</p>
                <p className="mt-0.5 text-[10px] text-zinc-500">
                  https://synbiosuite.org/
                </p>
              </div>
            </div>

            <h3 className="mt-6 text-lg font-semibold tracking-[-0.015em]">
              Sign in with your SBOL Identity
            </h3>
            <p className="mt-2 text-xs leading-5 text-zinc-400">
              SynBioSuite will receive:
            </p>
            <div className="mt-3 space-y-2 border-y border-white/10 py-3 text-xs text-zinc-300">
              <div className="flex items-center gap-2">
                <Check
                  className="size-3.5 text-emerald-300"
                  aria-hidden="true"
                />
                Name and SBOL DB profile
              </div>
              <div className="flex items-center gap-2">
                <Check
                  className="size-3.5 text-emerald-300"
                  aria-hidden="true"
                />
                Email address
              </div>
            </div>

            <div className="mt-4 flex h-9 items-center justify-center gap-2 rounded-[3px] bg-sky-300 px-4 text-xs font-semibold text-zinc-950 shadow-sm shadow-sky-500/20">
              <Fingerprint className="size-3.5" aria-hidden="true" />
              Continue with SBOL
            </div>
            <p className="mt-3 text-center text-[10px] leading-4 text-zinc-500">
              You can revoke access from your SBOL account.
            </p>
          </div>
        </div>
      </div>
    </section>
  );
}

function McpDocumentation({ serverAddress }: { serverAddress: string }) {
  const clipboard = useCopyToClipboard();
  const copyLabel = clipboard.copied
    ? "Server address copied"
    : clipboard.failed
      ? "Could not copy server address"
      : "Copy server address";

  return (
    <div
      id="mcp"
      className="mt-8 scroll-mt-24 overflow-hidden rounded-[4px] border border-foreground/15 bg-card"
    >
      <div className="grid lg:grid-cols-[1.35fr_0.65fr]">
        <div className="p-6 sm:p-8">
          <div className="flex items-start gap-3.5">
            <span className={`${QUIET_ICON_TILE} mt-0.5 size-10`}>
              <Cable className="size-[18px]" />
            </span>
            <div className="max-w-2xl">
              <p className="text-[11px] font-semibold uppercase tracking-[0.18em] text-primary">
                SBOL DB MCP
              </p>
              <h2 className="mt-1.5 text-balance text-2xl font-semibold tracking-[-0.025em]">
                Let your agent work safely with biological designs.
              </h2>
              <p className="mt-2 max-w-xl text-sm leading-6 text-muted-foreground">
                Find designs, prepare exact changes, and move reviews forward
                while staying inside your approved OAuth scopes and SBOL DB
                permissions.
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
              <h3 className="mt-2 text-lg font-semibold tracking-[-0.015em]">
                Add this registry as an MCP server.
              </h3>
              <p className="mt-2 text-xs leading-5 text-zinc-400">
                Use an MCP client that supports Streamable HTTP and browser
                OAuth.
              </p>
            </div>
          </div>

          <div className="mt-6 rounded-xl border border-white/10 bg-white/[0.04] p-4">
            <p className="text-[10px] font-medium uppercase tracking-[0.14em] text-zinc-500">
              Server address
            </p>
            <div className="mt-2 flex items-center gap-3">
              <code className="min-w-0 flex-1 break-all text-[13px] font-medium text-zinc-100">
                {serverAddress}
              </code>
              <button
                type="button"
                onClick={() => clipboard.copy(serverAddress)}
                aria-label={copyLabel}
                title={copyLabel}
                className="flex size-8 shrink-0 items-center justify-center rounded-[3px] bg-sky-300 text-zinc-950 shadow-sm shadow-sky-500/20 transition-[background-color,transform] duration-150 [transition-timing-function:var(--ease-out)] hover:bg-sky-200 active:scale-[0.97] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sky-300 focus-visible:ring-offset-2 focus-visible:ring-offset-zinc-950 motion-reduce:transition-none"
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
              title="Sign in on SBOL DB"
              description="The registry opens in your browser and names the capabilities your agent is requesting."
            />
            <ConnectionStep
              number="3"
              title="Approve the initial scope"
              description="Begin with read access. Additional capabilities are requested only when a task needs them."
            />
          </ol>

          <div className="mt-auto pt-6">
            <div className="rounded-xl border border-sky-300/10 bg-sky-300/[0.04] p-4">
              <div className="flex gap-3">
                <LockKeyhole className="mt-0.5 size-4 shrink-0 text-sky-300" />
                <div>
                  <p className="text-xs font-medium text-zinc-100">
                    Exact audience, exact account.
                  </p>
                  <p className="mt-1 text-xs leading-5 text-zinc-400">
                    API and identity tokens cannot be replayed at MCP. The grant
                    is bound to this server, OAuth client, account, and approved
                    scopes.
                  </p>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
      <PreparedChangeDocumentation />
    </div>
  );
}

function PreparedChangeDocumentation() {
  return (
    <div className="border-t border-foreground/15 bg-muted/15 p-6 sm:p-8">
      <div className="grid gap-7 lg:grid-cols-[0.72fr_1.28fr] lg:items-start">
        <div>
          <p className="ledger-label text-primary">Prepared changes</p>
          <h3 className="mt-3 text-balance text-2xl font-semibold tracking-[-0.025em]">
            Your agent proposes. You review. SBOL DB applies exactly that
            change.
          </h3>
          <p className="mt-3 text-sm leading-6 text-muted-foreground">
            Your agent's mutations are two-step workflows. Preparation validates
            the complete payload and returns a human-readable effect. Registry
            data does not change until the one-time plan is applied.
          </p>
        </div>
        <ol className="grid gap-px border bg-foreground/15 sm:grid-cols-3">
          <PreparedStep
            number="01"
            title="Prepare"
            description="Check permissions, content, identities, collisions, and the current design baseline."
          />
          <PreparedStep
            number="02"
            title="Review"
            description="Show the intended effect, input fingerprint, expiry, and opaque one-time plan token."
          />
          <PreparedStep
            number="03"
            title="Apply"
            description="Consume the stored payload once. Replay, substitution, expiry, or stale state fails closed."
          />
        </ol>
      </div>
    </div>
  );
}

function PreparedStep({
  number,
  title,
  description,
}: {
  number: string;
  title: string;
  description: string;
}) {
  return (
    <li className="bg-card p-4 sm:p-5">
      <span className="font-mono text-[10px] tracking-[0.16em] text-primary">
        {number}
      </span>
      <h4 className="mt-3 text-sm font-semibold">{title}</h4>
      <p className="mt-2 text-xs leading-5 text-muted-foreground">
        {description}
      </p>
    </li>
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
      <span className={`${QUIET_ICON_TILE} size-8`}>
        <Icon className="size-[15px]" />
      </span>
      <h3 className="mt-3 text-sm font-semibold tracking-[-0.015em]">
        {label}
      </h3>
      <p className="mt-1.5 text-xs leading-[1.55] text-muted-foreground">
        {description}
      </p>

      <ul className="mt-4 divide-y" role="list">
        {capabilities.map((capability) => (
          <li key={capability.title} className="py-2.5 first:pt-0 last:pb-0">
            <h4 className="text-xs font-semibold text-foreground">
              {capability.title}
            </h4>
            <p className="mt-1 text-xs leading-[1.45] text-muted-foreground">
              {capability.description}
            </p>
          </li>
        ))}
      </ul>
    </article>
  );
}
