import {
  Archive,
  Braces,
  Check,
  Copy,
  Dna,
  Download,
  ExternalLink,
  FileCode2,
  FileText,
  TriangleAlert,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useCopyToClipboard } from "@/hooks/useCopyToClipboard";

const formats = [
  {
    format: "sbol",
    version: "sbol3",
    label: "SBOL 3",
    description: "Recursive RDF/XML",
    icon: FileCode2,
  },
  {
    format: "sbol",
    version: "sbol2",
    label: "SBOL 2",
    description: "Compatible RDF/XML",
    icon: FileCode2,
  },
  {
    format: "sbolnr",
    version: "sbol3",
    label: "Non-recursive SBOL",
    description: "Root object only",
    icon: Braces,
  },
  {
    format: "fasta",
    label: "FASTA",
    description: "Sequence exchange",
    icon: Dna,
  },
  {
    format: "gb",
    label: "GenBank",
    description: "Annotated sequence",
    icon: FileText,
  },
  {
    format: "gff",
    label: "GFF3",
    description: "Feature annotations",
    icon: FileText,
  },
  {
    format: "omex",
    label: "OMEX archive",
    description: "Design with attachments",
    icon: Archive,
  },
] as const;

export function ObjectDownloads({ iri }: { iri: string }) {
  const base = `/api/v2/objects/${encodeURIComponent(iri)}`;
  return (
    <Card>
      <CardHeader className="border-b bg-muted/15 p-5">
        <CardTitle className="flex items-center gap-2 text-base">
          <Download className="size-4 text-primary" /> Download
        </CardTitle>
        <p className="text-xs leading-5 text-muted-foreground">
          Export the authorized object closure in a standard exchange format.
        </p>
      </CardHeader>
      <CardContent className="p-2">
        {formats.map((download) => {
          const Icon = download.icon;
          const query = new URLSearchParams({ format: download.format });
          if ("version" in download) query.set("version", download.version);
          return (
            <a
              key={`${download.format}-${"version" in download ? download.version : ""}`}
              href={`${base}?${query}`}
              download
              className="group flex min-h-12 items-center gap-3 rounded-lg px-3 py-2 transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              <span className="flex size-8 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground group-hover:bg-background group-hover:text-primary">
                <Icon className="size-3.5" />
              </span>
              <span className="min-w-0 flex-1">
                <span className="block text-sm font-medium">
                  {download.label}
                </span>
                <span className="block truncate text-[11px] text-muted-foreground">
                  {download.description}
                </span>
              </span>
              <Download className="size-3.5 text-muted-foreground/60 group-hover:text-primary" />
            </a>
          );
        })}
      </CardContent>
    </Card>
  );
}

export function MachineAccess({ iri }: { iri: string }) {
  const clipboard = useCopyToClipboard();
  const resourcePath = `/api/v2/objects/${encodeURIComponent(iri)}/details`;

  const copyResource = () =>
    clipboard.copy(new URL(resourcePath, window.location.origin).href);

  return (
    <Card>
      <CardHeader className="p-5 pb-3">
        <CardTitle className="text-base">Machine access</CardTitle>
      </CardHeader>
      <CardContent className="space-y-3 px-5 pb-5">
        <p className="text-xs leading-5 text-muted-foreground">
          Read the normalized V2 object-details resource or copy its URL into a
          client.
        </p>
        <code className="block truncate rounded-md bg-muted px-2.5 py-2 text-[10px] text-muted-foreground">
          {resourcePath}
        </code>
        <div className="grid grid-cols-2 gap-2">
          <Button variant="outline" size="sm" onClick={copyResource}>
            {clipboard.copied ? (
              <Check />
            ) : clipboard.failed ? (
              <TriangleAlert />
            ) : (
              <Copy />
            )}
            {clipboard.copied
              ? "Copied"
              : clipboard.failed
                ? "Try again"
                : "Copy URL"}
          </Button>
          <Button asChild variant="outline" size="sm">
            <a href={resourcePath} target="_blank" rel="noopener noreferrer">
              <ExternalLink /> Open JSON
            </a>
          </Button>
        </div>
        <span className="sr-only" aria-live="polite">
          {clipboard.copied
            ? "API resource URL copied to clipboard"
            : clipboard.failed
              ? "Could not copy the API resource URL"
              : ""}
        </span>
      </CardContent>
    </Card>
  );
}
