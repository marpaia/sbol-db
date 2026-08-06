/**
 * Backend-neutral RDF resource detail. Metadata is projected from canonical
 * triples and every named-graph occurrence remains visible.
 */

import { useQueryClient } from "@tanstack/react-query";
import {
  ChevronLeft,
  Copy,
  ExternalLink,
  GitBranch,
  TriangleAlert,
} from "lucide-react";
import { Link, useNavigate, useParams } from "react-router-dom";

import { AdminPage } from "@/components/admin/AdminPage";
import { ErrorBanner } from "@/components/lab/ErrorBanner";
import { Button } from "@/components/ui/button";
import { useObjectByIri } from "@/features/admin/objects/queries";
import type { SbolObjectRecord } from "@/features/admin/objects/api";
import type { CatalogResourceOccurrence } from "@/features/admin/api";
import { HttpError } from "@/shared/api/http";
import { adminPath, publicObjectPath } from "@/lib/routes";

export default function ObjectDetailRoute() {
  const params = useParams<{ iri: string }>();
  const iri = decodeURIComponent(params.iri ?? "");
  const navigate = useNavigate();
  const { data, error, isLoading } = useObjectByIri(iri);
  const object = data?.resource;
  const title =
    firstValue(object?.meta.name) ||
    firstValue(object?.meta.display_id) ||
    "Technical resource inspector";

  return (
    <AdminPage
      title={title}
      description="Canonical RDF resource properties and every named graph in which they occur."
      eyebrow="Data model · Technical inspector"
      maxWidth="5xl"
      action={
        object ? (
          <Button asChild variant="outline" size="sm">
            <Link to={publicObjectPath(object.iri)}>
              <ExternalLink />
              Open registry view
            </Link>
          </Button>
        ) : undefined
      }
    >
      <Link
        to={adminPath("/objects")}
        className="inline-flex items-center gap-1 text-xs text-muted-foreground transition-colors hover:text-foreground"
      >
        <ChevronLeft size={12} />
        Resource browser
      </Link>

      {error instanceof HttpError && error.status === 404 ? (
        <NotFound iri={iri} />
      ) : error ? (
        <ErrorBanner
          title="Couldn't load resource"
          body={(error as Error).message}
        />
      ) : isLoading || !object || !data ? (
        <Skeleton />
      ) : (
        <>
          <Header object={object} />
          <Actions
            object={object}
            onNeighborhood={() =>
              navigate(
                adminPath(`/neighborhood?iri=${encodeURIComponent(object.iri)}`)
              )
            }
          />
          <Properties object={object} />
          <Occurrences occurrences={data.occurrences} />
          <RawData object={object} />
        </>
      )}
    </AdminPage>
  );
}

function Header({ object }: { object: SbolObjectRecord }) {
  const queryClient = useQueryClient();
  return (
    <section className="rounded-lg border bg-card px-4 py-3">
      <div className="flex items-center gap-2">
        <div className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted-foreground/80">
          {object.iri}
        </div>
        <button
          type="button"
          onClick={() => {
            void navigator.clipboard.writeText(object.iri);
            queryClient.setQueryData(["lab", "copied-toast"], Date.now());
          }}
          className="text-muted-foreground transition-colors hover:text-foreground"
          aria-label="Copy IRI"
          title="Copy IRI"
        >
          <Copy size={14} />
        </button>
      </div>
      {(object.meta.types?.length ?? 0) > 0 && (
        <div className="mt-1 font-mono text-[11px] text-muted-foreground">
          a <span className="text-foreground">{object.meta.types?.[0]}</span>
        </div>
      )}
    </section>
  );
}

function Actions({
  object,
  onNeighborhood,
}: {
  object: SbolObjectRecord;
  onNeighborhood: () => void;
}) {
  const isHttp = /^https?:\/\//i.test(object.iri);

  return (
    <section className="flex flex-wrap items-center gap-2 rounded-lg border bg-card px-4 py-3">
      <button
        type="button"
        onClick={onNeighborhood}
        className="inline-flex items-center gap-1.5 rounded-md bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90"
      >
        <GitBranch size={14} />
        Walk neighborhood
      </button>

      {isHttp && (
        <a
          href={object.iri}
          target="_blank"
          rel="noopener noreferrer"
          className="ml-auto inline-flex items-center gap-1.5 text-xs text-muted-foreground transition-colors hover:text-foreground"
        >
          <ExternalLink size={12} />
          Open IRI
        </a>
      )}

    </section>
  );
}

function Properties({ object }: { object: SbolObjectRecord }) {
  return (
    <section className="rounded-lg border bg-card px-4 py-3">
      <dl className="grid gap-3 text-sm sm:grid-cols-2">
        <Pair label="Display ID" value={firstValue(object.meta.display_id)} />
        <Pair label="Name" value={firstValue(object.meta.name)} />
        <Pair label="Version" value={firstValue(object.meta.version)} />
        <Pair label="Named graphs" value={object.graph_count.toLocaleString()} />
        <PairList label="RDF classes" values={object.meta.types ?? []} />
        <PairList label="SBOL types" values={object.meta.sbol_types ?? []} />
        <PairList label="Roles" values={object.meta.roles ?? []} />
        <PairList label="Creators" values={object.meta.creators ?? []} />
      </dl>
    </section>
  );
}

function Pair({
  label,
  value,
  mono,
}: {
  label: string;
  value: string | null | undefined;
  mono?: boolean;
}) {
  return (
    <div>
      <dt className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
        {label}
      </dt>
      <dd
        className={`mt-0.5 truncate text-foreground ${
          mono ? "font-mono text-[11px]" : "text-sm"
        }`}
      >
        {value || <span className="text-muted-foreground/60">—</span>}
      </dd>
    </div>
  );
}

function PairList({ label, values }: { label: string; values: string[] }) {
  return (
    <div>
      <dt className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
        {label} ({values.length})
      </dt>
      <dd className="mt-1 flex flex-wrap gap-1.5">
        {values.length === 0 ? (
          <span className="text-sm text-muted-foreground/60">—</span>
        ) : (
          values.map((v) => (
            <span
              key={v}
              title={v}
              className="rounded-sm border bg-background px-1.5 py-0.5 font-mono text-[10px] text-foreground"
            >
              {shortIri(v)}
            </span>
          ))
        )}
      </dd>
    </div>
  );
}

function RawData({ object }: { object: SbolObjectRecord }) {
  const json = JSON.stringify(object.meta, null, 2);
  return (
    <section>
      <SectionLabel>Raw projection</SectionLabel>
      <pre className="max-h-96 overflow-auto rounded-lg border bg-card px-4 py-3 font-mono text-[11px] text-foreground">
        {json}
      </pre>
    </section>
  );
}

function Occurrences({
  occurrences,
}: {
  occurrences: CatalogResourceOccurrence[];
}) {
  return (
    <section>
      <SectionLabel>Named graph occurrences ({occurrences.length})</SectionLabel>
      <ul className="divide-y rounded-lg border bg-card px-4">
        {occurrences.map((occurrence) => (
          <li
            key={occurrence.graph_iri}
            className="py-2 font-mono text-[11px] text-foreground"
          >
            {occurrence.graph_iri}
          </li>
        ))}
      </ul>
    </section>
  );
}

function NotFound({ iri }: { iri: string }) {
  return (
    <div className="flex items-start gap-3 rounded-md border bg-muted/40 px-3 py-3 text-sm">
      <TriangleAlert
        size={14}
        className="mt-0.5 shrink-0 text-muted-foreground"
        aria-hidden
      />
      <div>
        <div className="font-medium text-foreground">Resource not found</div>
        <div className="mt-0.5 text-muted-foreground">
          No typed RDF resource at <code className="font-mono">{iri}</code>.
        </div>
      </div>
    </div>
  );
}

function Skeleton() {
  return (
    <div className="space-y-3">
      <div className="h-12 animate-pulse rounded-md bg-card" />
      <div className="h-16 animate-pulse rounded-md bg-card" />
      <div className="h-48 animate-pulse rounded-md bg-card" />
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

function shortIri(iri: string): string {
  const m = iri.match(/[#/]([^#/]+)$/);
  return m ? m[1] : iri;
}

function firstValue(values?: Array<{ value: string }>): string {
  return values?.[0]?.value ?? "";
}
