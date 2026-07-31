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

import { shortIri } from "@/features/portal/format";
import { cn } from "@/lib/utils";

export function ObjectTypeMark({
  objectType,
  className,
}: {
  objectType: string | null | undefined;
  className?: string;
}) {
  const Icon = iconForObjectType(objectType);
  return (
    <span
      className={cn(
        "flex shrink-0 items-center justify-center rounded-xl bg-primary/10 text-primary ring-1 ring-inset ring-primary/10",
        className
      )}
      aria-hidden="true"
    >
      <Icon className="size-[42%]" strokeWidth={1.8} />
    </span>
  );
}

function iconForObjectType(objectType: string | null | undefined) {
  const type = shortIri(objectType).toLowerCase();
  if (type.includes("collection")) return FolderArchive;
  if (type.includes("sequence")) return Braces;
  if (type.includes("component")) return Dna;
  if (type.includes("interaction") || type.includes("participation")) {
    return Network;
  }
  if (type.includes("experiment") || type.includes("implementation")) {
    return FlaskConical;
  }
  if (type.includes("attachment")) return Paperclip;
  if (type.includes("module") || type.includes("system")) return Boxes;
  return Box;
}
