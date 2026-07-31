import { Activity, Boxes, Check, Copy, Dna, TriangleAlert } from "lucide-react";

import { ObjectRelationGroup } from "@/components/portal/ObjectRelationGroup";
import { ObjectSection } from "@/components/portal/ObjectSection";
import { ObjectStateBadge } from "@/components/portal/ObjectStateBadge";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { PortalObjectDetails } from "@/features/portal/api";
import { shortIri } from "@/features/portal/format";
import { useCopyToClipboard } from "@/hooks/useCopyToClipboard";

export function ObjectBiology({ object }: { object: PortalObjectDetails }) {
  return (
    <ObjectSection
      id="biology"
      icon={Dna}
      title="Biological structure"
      description="Sequence, feature, and interaction assertions from the selected authorized graph."
    >
      {object.sequence_content.state !== "unsupported" && (
        <SequenceContent object={object} />
      )}
      <div className="grid gap-4 lg:grid-cols-3">
        <ObjectRelationGroup
          icon={Dna}
          title="Sequences"
          description="Sequence resources asserted by this design."
          section={object.sequences}
          emptyLabel="No sequence resources are asserted."
        />
        <ObjectRelationGroup
          icon={Boxes}
          title="Features"
          description="Addressable component or sequence features."
          section={object.features}
          emptyLabel="No feature resources are asserted."
        />
        <ObjectRelationGroup
          icon={Activity}
          title="Interactions"
          description="Interaction resources owned by this design."
          section={object.interactions}
          emptyLabel="No interaction resources are asserted."
        />
      </div>
    </ObjectSection>
  );
}

function SequenceContent({ object }: { object: PortalObjectDetails }) {
  const sequence = object.sequence_content;
  const clipboard = useCopyToClipboard();
  return (
    <div className="mb-5 overflow-hidden rounded-xl border bg-muted/10">
      <div className="flex flex-wrap items-center justify-between gap-3 border-b bg-background/70 px-4 py-3">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm font-semibold">Sequence elements</span>
          <ObjectStateBadge state={sequence.state} />
          {sequence.length !== null && (
            <Badge variant="secondary" className="text-[10px] tabular-nums">
              {sequence.length.toLocaleString()} bases
            </Badge>
          )}
          {sequence.encoding && (
            <Badge
              variant="outline"
              className="max-w-56 truncate font-mono text-[10px]"
              title={sequence.encoding}
            >
              {shortIri(sequence.encoding)}
            </Badge>
          )}
        </div>
        {sequence.elements && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => clipboard.copy(sequence.elements || "")}
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
                : "Copy sequence"}
          </Button>
        )}
      </div>
      {sequence.elements ? (
        <pre className="max-h-48 overflow-auto whitespace-pre-wrap break-all p-4 font-mono text-xs leading-6 tracking-[0.08em] text-foreground/85">
          {sequence.elements}
        </pre>
      ) : (
        <p className="px-4 py-5 text-xs leading-5 text-muted-foreground">
          {sequence.note || "No sequence elements are asserted."}
        </p>
      )}
      {sequence.note && sequence.elements && (
        <p className="border-t bg-amber-500/5 px-4 py-2.5 text-xs text-amber-800 dark:text-amber-200">
          {sequence.note}
        </p>
      )}
      <span className="sr-only" aria-live="polite">
        {clipboard.copied
          ? "Sequence copied to clipboard"
          : clipboard.failed
            ? "Could not copy the sequence"
            : ""}
      </span>
    </div>
  );
}
