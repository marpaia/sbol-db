import {
  ArrowLeft,
  Check,
  Copy,
  Globe2,
  LockKeyhole,
  TriangleAlert,
} from "lucide-react";
import { Link } from "react-router-dom";

import { ObjectTypeMark } from "@/components/portal/ObjectTypeMark";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { PortalObjectDetails } from "@/features/portal/api";
import { shortIri } from "@/features/portal/format";
import { useCopyToClipboard } from "@/hooks/useCopyToClipboard";

export function ObjectHeader({ object }: { object: PortalObjectDetails }) {
  const clipboard = useCopyToClipboard();
  const title = object.name || object.display_id || shortIri(object.iri);
  const additionalTypes = object.types.filter(
    (type) => type !== object.object_type
  );

  return (
    <header className="registry-field overflow-hidden border-b border-foreground/15">
      <div className="mx-auto max-w-[90rem] px-4 pb-10 pt-7 sm:px-6 sm:pb-12 lg:px-8">
        <Link
          to="/search"
          className="mb-8 inline-flex min-h-9 items-center gap-1.5 rounded-[3px] pr-2 text-sm text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
        >
          <ArrowLeft className="size-4" /> Back to search
        </Link>
        <div className="flex items-start gap-4 sm:gap-5">
          <ObjectTypeMark
            objectType={object.object_type}
            className="mt-0.5 size-12 bg-card/75 sm:size-14"
          />
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <Badge className="rounded-[2px] font-mono text-[10px] uppercase tracking-[0.08em]">
                {shortIri(object.object_type)}
              </Badge>
              <Badge
                variant="outline"
                className="gap-1.5 rounded-[2px] bg-background/70 text-[10px]"
              >
                {object.visibility === "public" ? (
                  <Globe2 className="size-3" />
                ) : (
                  <LockKeyhole className="size-3" />
                )}
                {object.visibility === "public" ? "Public" : "Restricted"}
              </Badge>
              {additionalTypes.slice(0, 2).map((type) => (
                <Badge
                  key={type}
                  variant="outline"
                  className="max-w-full rounded-[2px] font-mono text-[10px]"
                  title={type}
                >
                  <span className="truncate">{shortIri(type)}</span>
                </Badge>
              ))}
              {additionalTypes.length > 2 && (
                <Badge variant="outline" className="text-[10px]">
                  +{additionalTypes.length - 2} types
                </Badge>
              )}
            </div>

            <h1 className="record-title mt-5 max-w-4xl text-balance text-4xl font-medium leading-tight tracking-[-0.035em] sm:text-5xl">
              {title}
            </h1>
            {object.description ? (
              <p className="mt-4 max-w-3xl text-pretty text-base leading-7 text-muted-foreground">
                {object.description}
              </p>
            ) : (
              <p className="mt-4 text-sm text-muted-foreground">
                No human-readable description has been provided for this object.
              </p>
            )}

            <div className="mt-7 flex max-w-4xl items-center gap-2 border-y border-foreground/20 bg-background/70 py-2 pl-3">
              <code className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                {object.iri}
              </code>
              <Button
                variant="ghost"
                size="sm"
                className="shrink-0"
                onClick={() => clipboard.copy(object.iri)}
                aria-label={
                  clipboard.copied
                    ? "IRI copied"
                    : clipboard.failed
                      ? "Copy failed; try copying the IRI again"
                      : "Copy object IRI"
                }
              >
                {clipboard.copied ? (
                  <Check />
                ) : clipboard.failed ? (
                  <TriangleAlert />
                ) : (
                  <Copy />
                )}
                <span className="hidden sm:inline">
                  {clipboard.copied
                    ? "Copied"
                    : clipboard.failed
                      ? "Try again"
                      : "Copy IRI"}
                </span>
              </Button>
              <span className="sr-only" aria-live="polite">
                {clipboard.copied
                  ? "Object IRI copied to clipboard"
                  : clipboard.failed
                    ? "Could not copy the object IRI"
                    : ""}
              </span>
            </div>
          </div>
        </div>
      </div>
    </header>
  );
}
