import { useState } from "react";
import { Braces, Check, ChevronDown, Copy, TriangleAlert } from "lucide-react";

import { ObjectSection } from "@/components/portal/ObjectSection";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent } from "@/components/ui/collapsible";
import type { PortalObjectDetails } from "@/features/portal/api";
import { useCopyToClipboard } from "@/hooks/useCopyToClipboard";
import { cn } from "@/lib/utils";

export function ObjectRawProjection({
  object,
}: {
  object: PortalObjectDetails;
}) {
  const [open, setOpen] = useState(false);
  const clipboard = useCopyToClipboard();
  const json = JSON.stringify(
    {
      "@id": object.iri,
      "@type": object.types,
      properties: object.properties,
    },
    null,
    2
  );

  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      <ObjectSection
        id="raw-projection"
        icon={Braces}
        title="Normalized RDF projection"
        description="Lossless resource, blank-node, literal, datatype, and language terms from the selected authorized graph; this is not JSON-LD."
        action={
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setOpen((value) => !value)}
            aria-expanded={open}
          >
            {open ? "Hide" : "Inspect"}
            <ChevronDown
              className={cn(
                "transition-transform duration-150 motion-reduce:transition-none",
                open && "rotate-180"
              )}
            />
          </Button>
        }
      >
        <p className={cn("text-sm text-muted-foreground", open && "sr-only")}>
          Expand this section when you need the exact normalized RDF terms.
        </p>
        <CollapsibleContent>
          <div className="mb-3 flex justify-end">
            <Button
              variant="outline"
              size="sm"
              onClick={() => clipboard.copy(json)}
            >
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
                  : "Copy JSON"}
            </Button>
          </div>
          <pre className="max-h-[38rem] overflow-auto rounded-lg border bg-muted/35 p-4 font-mono text-[11px] leading-6">
            {json}
          </pre>
          <span className="sr-only" aria-live="polite">
            {clipboard.copied
              ? "Raw JSON copied to clipboard"
              : clipboard.failed
                ? "Could not copy the raw JSON"
                : ""}
          </span>
        </CollapsibleContent>
      </ObjectSection>
    </Collapsible>
  );
}
