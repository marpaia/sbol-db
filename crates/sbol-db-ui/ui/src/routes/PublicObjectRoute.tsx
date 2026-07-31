import { useState } from "react";
import { ArrowLeft, Check, Copy, Download, ExternalLink } from "lucide-react";
import { Link, useParams } from "react-router-dom";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { PortalApiError } from "@/features/portal/api";
import { shortIri } from "@/features/portal/format";
import { usePortalObject } from "@/features/portal/queries";

const downloads = [
  { format: "sbol", label: "SBOL RDF" },
  { format: "sbolnr", label: "SBOL (non-recursive)" },
  { format: "fasta", label: "FASTA" },
  { format: "gb", label: "GenBank" },
] as const;

export default function PublicObjectRoute() {
  const params = useParams<{ iri: string }>();
  const iri = decodeURIComponent(params.iri || "");
  const object = usePortalObject(iri);
  const [copied, setCopied] = useState(false);

  if (object.isLoading) {
    return (
      <div className="mx-auto max-w-5xl space-y-5 px-4 py-12 sm:px-6 lg:px-8">
        <Skeleton className="h-5 w-28" />
        <Skeleton className="h-11 w-2/3" />
        <Skeleton className="h-28 w-full rounded-xl" />
        <Skeleton className="h-72 w-full rounded-xl" />
      </div>
    );
  }

  if (object.error || !object.data) {
    const missing =
      object.error instanceof PortalApiError && object.error.status === 404;
    return (
      <div className="mx-auto max-w-3xl px-4 py-20 text-center sm:px-6">
        <h1 className="text-2xl font-semibold">
          {missing ? "Design not found" : "Couldn’t load this design"}
        </h1>
        <p className="mt-3 text-sm leading-6 text-muted-foreground">
          {missing
            ? "It may not exist, or it may be outside the graphs visible to your account."
            : (object.error as Error)?.message}
        </p>
        <Button asChild variant="outline" className="mt-6">
          <Link to="/search">Return to search</Link>
        </Button>
      </div>
    );
  }

  const data = object.data;
  const title = data.name || data.display_id || shortIri(data.iri);
  const downloadBase = `/api/v2/objects/${encodeURIComponent(data.iri)}`;

  const copyIri = async () => {
    await navigator.clipboard.writeText(data.iri);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1600);
  };

  return (
    <div className="mx-auto max-w-6xl px-4 py-10 sm:px-6 lg:px-8">
      <Link
        to="/search"
        className="inline-flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground"
      >
        <ArrowLeft className="size-4" /> Back to search
      </Link>

      <header className="mt-8 border-b pb-8">
        <div className="flex flex-wrap items-center gap-2">
          <Badge>{shortIri(data.sbol_class)}</Badge>
          {data.types.slice(0, 2).map((type) => (
            <Badge
              key={type}
              variant="outline"
              className="font-mono text-[10px]"
            >
              {shortIri(type)}
            </Badge>
          ))}
        </div>
        <div className="mt-4 flex items-start gap-3">
          <div className="min-w-0 flex-1">
            <h1 className="text-balance text-3xl font-semibold tracking-tight sm:text-4xl">
              {title}
            </h1>
            {data.description && (
              <p className="mt-4 max-w-3xl text-base leading-7 text-muted-foreground">
                {data.description}
              </p>
            )}
          </div>
          <Button variant="outline" size="sm" onClick={copyIri}>
            {copied ? <Check /> : <Copy />}
            {copied ? "Copied" : "Copy IRI"}
          </Button>
        </div>
        <div className="mt-5 break-all rounded-lg bg-muted/50 px-3 py-2 font-mono text-xs text-muted-foreground">
          {data.iri}
        </div>
      </header>

      <div className="mt-8 grid gap-6 lg:grid-cols-[minmax(0,1fr)_19rem]">
        <div className="space-y-6">
          <Card>
            <CardHeader>
              <CardTitle>Identity and classification</CardTitle>
            </CardHeader>
            <CardContent>
              <dl className="grid gap-x-8 gap-y-5 sm:grid-cols-2">
                <Property label="Display ID" value={data.display_id} />
                <Property label="SBOL class" value={data.sbol_class} mono />
                <Property label="Graph" value={data.graph_id} mono />
                <Property label="Object ID" value={data.id} mono />
              </dl>
              {data.roles.length > 0 && (
                <div className="mt-7 border-t pt-5">
                  <div className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
                    Roles
                  </div>
                  <div className="mt-3 flex flex-wrap gap-2">
                    {data.roles.map((role) => (
                      <Badge
                        key={role}
                        variant="secondary"
                        className="font-mono text-[10px]"
                      >
                        {shortIri(role)}
                      </Badge>
                    ))}
                  </div>
                </div>
              )}
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Stored SBOL data</CardTitle>
            </CardHeader>
            <CardContent>
              <pre className="max-h-[34rem] overflow-auto rounded-lg bg-muted/50 p-4 font-mono text-xs leading-6">
                {JSON.stringify(data.data, null, 2)}
              </pre>
            </CardContent>
          </Card>
        </div>

        <aside className="space-y-4">
          <Card>
            <CardHeader className="pb-4">
              <CardTitle className="text-base">Download</CardTitle>
            </CardHeader>
            <CardContent className="space-y-1.5">
              {downloads.map((download) => (
                <Button
                  key={download.format}
                  asChild
                  variant="ghost"
                  className="w-full justify-start"
                >
                  <a href={`${downloadBase}?format=${download.format}`}>
                    <Download /> {download.label}
                  </a>
                </Button>
              ))}
            </CardContent>
          </Card>
          <Card>
            <CardHeader className="pb-4">
              <CardTitle className="text-base">Machine access</CardTitle>
            </CardHeader>
            <CardContent className="space-y-3 text-sm text-muted-foreground">
              <p>Fetch this object through the stable V2 resource API.</p>
              <Button asChild variant="outline" className="w-full">
                <a
                  href={downloadBase}
                  target="_blank"
                  rel="noopener noreferrer"
                >
                  <ExternalLink /> Open JSON
                </a>
              </Button>
            </CardContent>
          </Card>
        </aside>
      </div>
    </div>
  );
}

function Property({
  label,
  value,
  mono,
}: {
  label: string;
  value?: string | null;
  mono?: boolean;
}) {
  return (
    <div className="min-w-0">
      <dt className="text-xs font-medium text-muted-foreground">{label}</dt>
      <dd
        className={mono ? "mt-1 break-all font-mono text-xs" : "mt-1 text-sm"}
      >
        {value || (
          <span className="text-muted-foreground/60">Not provided</span>
        )}
      </dd>
    </div>
  );
}
