import {
  Box,
  Boxes,
  Braces,
  Dna,
  FlaskConical,
  FolderArchive,
  Network,
  Paperclip,
} from "lucide-react";

import { shortIri } from "@/features/registry/objects/format";
import { cn } from "@/lib/utils";

export function ObjectTypeMark({
  objectType,
  className,
}: {
  objectType: string | null | undefined;
  className?: string;
}) {
  const { Icon, tone } = visualForObjectType(objectType);
  return (
    <span
      className={cn(
        "flex shrink-0 items-center justify-center border border-foreground/15 border-l-2 bg-background/60",
        tone,
        className
      )}
      aria-hidden="true"
    >
      <Icon className="size-[42%]" strokeWidth={1.8} />
    </span>
  );
}

function visualForObjectType(objectType: string | null | undefined) {
  const type = shortIri(objectType).toLowerCase();
  if (type.includes("collection")) {
    return { Icon: FolderArchive, tone: "border-l-sbol-rbs text-sbol-rbs" };
  }
  if (type.includes("sequence")) {
    return { Icon: Braces, tone: "border-l-sbol-promoter text-sbol-promoter" };
  }
  if (type.includes("component")) {
    return { Icon: Dna, tone: "border-l-sbol-cds text-sbol-cds" };
  }
  if (type.includes("interaction") || type.includes("participation")) {
    return {
      Icon: Network,
      tone: "border-l-sbol-terminator text-sbol-terminator",
    };
  }
  if (type.includes("experiment") || type.includes("implementation")) {
    return { Icon: FlaskConical, tone: "border-l-sbol-rbs text-sbol-rbs" };
  }
  if (type.includes("attachment")) {
    return { Icon: Paperclip, tone: "border-l-primary text-primary" };
  }
  if (type.includes("module") || type.includes("system")) {
    return { Icon: Boxes, tone: "border-l-sbol-cds text-sbol-cds" };
  }
  return { Icon: Box, tone: "border-l-muted-foreground text-muted-foreground" };
}
