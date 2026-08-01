import { Download, ExternalLink, FileArchive } from "lucide-react";
import { Link } from "react-router-dom";

import { ObjectSection } from "@/components/portal/ObjectSection";
import { ObjectStateBadge } from "@/components/portal/ObjectStateBadge";
import { SurfaceState } from "@/components/portal/SurfaceState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { PortalObjectDetails } from "@/features/portal/api";
import { shortIri } from "@/features/portal/format";
import { publicObjectPath } from "@/lib/routes";

export function ObjectAttachments({ object }: { object: PortalObjectDetails }) {
  const section = object.attachments;
  return (
    <ObjectSection
      id="attachments"
      icon={FileArchive}
      title="Attachments"
      description="Files and external resources explicitly attached to this object."
      action={<ObjectStateBadge state={section.state} />}
    >
      {section.items.length === 0 ? (
        <SurfaceState
          title="No attachments are asserted"
          description="The object has no visible SBOL2, SBOL3, or migrated registry attachment relationships."
        />
      ) : (
        <div className="grid gap-3 sm:grid-cols-2">
          {section.items.map((attachment) => {
            const safeSource = safeAttachmentSource(attachment.source);
            return (
              <article
                key={attachment.uri}
                className="flex min-w-0 flex-col rounded-xl border bg-card p-4"
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <h3 className="truncate text-sm font-semibold">
                      {attachment.name || shortIri(attachment.uri)}
                    </h3>
                    <p
                      className="mt-1 truncate font-mono text-[10px] text-muted-foreground"
                      title={attachment.uri}
                    >
                      {attachment.uri}
                    </p>
                  </div>
                  {!attachment.resolved && (
                    <Badge
                      variant="outline"
                      className="border-dashed text-[10px]"
                    >
                      Metadata missing
                    </Badge>
                  )}
                </div>
                <div className="mt-4 flex flex-wrap gap-2">
                  {attachment.format && (
                    <Badge
                      variant="secondary"
                      className="font-mono text-[10px]"
                    >
                      {shortIri(attachment.format)}
                    </Badge>
                  )}
                  {attachment.size !== null && (
                    <Badge
                      variant="outline"
                      className="text-[10px] tabular-nums"
                    >
                      {formatBytes(attachment.size)}
                    </Badge>
                  )}
                </div>
                {attachment.hash && (
                  <code
                    className="mt-3 truncate rounded-md bg-muted px-2.5 py-2 font-mono text-[10px] text-muted-foreground"
                    title={attachment.hash}
                  >
                    {attachment.hash}
                  </code>
                )}
                <div className="mt-auto flex gap-2 pt-4">
                  <Button
                    asChild
                    variant="outline"
                    size="sm"
                    className="flex-1"
                  >
                    <Link to={publicObjectPath(attachment.uri)}>
                      View object
                    </Link>
                  </Button>
                  {safeSource && (
                    <Button
                      asChild
                      variant="outline"
                      size="sm"
                      className="flex-1"
                    >
                      <a
                        href={safeSource}
                        target="_blank"
                        rel="noopener noreferrer"
                      >
                        {safeSource.endsWith("/download") ? (
                          <Download />
                        ) : (
                          <ExternalLink />
                        )}
                        Open
                      </a>
                    </Button>
                  )}
                </div>
              </article>
            );
          })}
        </div>
      )}
      {section.note && (
        <p className="mt-4 rounded-lg border border-amber-500/20 bg-amber-500/5 px-3 py-2 text-xs leading-5 text-amber-800 dark:text-amber-200">
          {section.note}
        </p>
      )}
    </ObjectSection>
  );
}

function safeAttachmentSource(source: string | null): string | null {
  if (!source) return null;
  if (source.startsWith("/") && !source.startsWith("//")) return source;
  try {
    const url = new URL(source);
    return url.protocol === "http:" || url.protocol === "https:"
      ? source
      : null;
  } catch {
    return null;
  }
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = units[0];
  for (let index = 1; value >= 1024 && index < units.length; index += 1) {
    value /= 1024;
    unit = units[index];
  }
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${unit}`;
}
