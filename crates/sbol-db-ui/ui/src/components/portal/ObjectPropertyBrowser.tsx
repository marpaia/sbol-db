import { useMemo, useState } from "react";
import { ArrowUpRight, Braces, Link2 } from "lucide-react";
import { Link } from "react-router-dom";

import { ObjectSection } from "@/components/portal/ObjectSection";
import { SurfaceState } from "@/components/portal/SurfaceState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type {
  ObjectPropertyValue,
  PortalObjectDetails,
} from "@/features/portal/api";
import { shortIri } from "@/features/portal/format";
import { propertyLabel } from "@/features/portal/object-presentation";
import { publicObjectPath } from "@/lib/routes";

const INITIAL_PROPERTIES = 8;

export function ObjectPropertyBrowser({
  object,
}: {
  object: PortalObjectDetails;
}) {
  const properties = useMemo(() => {
    return object.properties
      .map((property) => ({
        iri: property.predicate,
        label: propertyLabel(property.predicate),
        values: property.values,
        resourceCount: property.values.filter(
          (value) => value.kind === "resource"
        ).length,
        literalCount: property.values.filter(
          (value) => value.kind === "literal"
        ).length,
      }))
      .sort(
        (left, right) =>
          left.label.localeCompare(right.label, undefined, {
            sensitivity: "base",
          }) || left.iri.localeCompare(right.iri)
      );
  }, [object.properties]);
  const [expanded, setExpanded] = useState(false);
  const visible = expanded
    ? properties
    : properties.slice(0, INITIAL_PROPERTIES);
  const relationships = properties.reduce(
    (total, property) => total + property.resourceCount,
    0
  );
  const literals = properties.reduce(
    (total, property) => total + property.literalCount,
    0
  );

  return (
    <ObjectSection
      id="properties"
      icon={Link2}
      title="Relationships and properties"
      description="The complete stored property projection, kept vocabulary-neutral so extension terms remain visible."
      action={
        <div className="hidden items-center gap-2 sm:flex">
          <Badge variant="outline" className="whitespace-nowrap text-[10px]">
            {relationships} links
          </Badge>
          <Badge variant="outline" className="whitespace-nowrap text-[10px]">
            {literals} values
          </Badge>
        </div>
      }
      contentClassName="p-0 sm:p-0"
    >
      {properties.length === 0 ? (
        <SurfaceState
          title="No projected properties"
          description="This object record contains identity and classification metadata, but no additional projected RDF properties."
          className="m-5 sm:m-6"
        />
      ) : (
        <>
          <dl className="divide-y">
            {visible.map((property) => (
              <div
                key={property.iri}
                className="grid gap-3 px-5 py-4 sm:grid-cols-[12rem_minmax(0,1fr)] sm:gap-6 sm:px-6"
              >
                <dt className="min-w-0">
                  <div className="text-sm font-medium">{property.label}</div>
                  <div
                    className="mt-1 truncate font-mono text-[10px] text-muted-foreground"
                    title={property.iri}
                  >
                    {shortIri(property.iri)}
                  </div>
                </dt>
                <dd className="min-w-0 space-y-2">
                  {property.values.map((value, index) => (
                    <PropertyValue
                      key={`${property.iri}-${index}`}
                      value={value}
                    />
                  ))}
                </dd>
              </div>
            ))}
          </dl>
          {properties.length > INITIAL_PROPERTIES && (
            <div className="border-t bg-muted/10 px-5 py-3 sm:px-6">
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setExpanded((value) => !value)}
                aria-expanded={expanded}
              >
                {expanded
                  ? "Show fewer properties"
                  : `Show all ${properties.length} properties`}
              </Button>
            </div>
          )}
        </>
      )}
    </ObjectSection>
  );
}

function PropertyValue({ value }: { value: ObjectPropertyValue }) {
  if (value.kind === "resource") {
    return (
      <Link
        to={publicObjectPath(value.value)}
        className="group/value flex min-w-0 items-start gap-2 rounded-md border bg-muted/15 px-3 py-2 text-xs transition-colors hover:border-primary/30 hover:bg-accent/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        title={value.value}
      >
        <ArrowUpRight className="mt-0.5 size-3.5 shrink-0 text-primary" />
        <span className="min-w-0">
          <span className="block font-medium group-hover/value:text-primary">
            {shortIri(value.value)}
          </span>
          <span className="mt-0.5 block truncate font-mono text-[10px] text-muted-foreground">
            {value.value}
          </span>
        </span>
      </Link>
    );
  }

  if (value.kind === "literal") {
    return (
      <div className="rounded-md bg-muted/35 px-3 py-2.5">
        <div className="break-words text-sm leading-6">{value.value}</div>
        {(value.language || value.datatype) && (
          <div className="mt-1.5 flex flex-wrap gap-1.5 font-mono text-[10px] text-muted-foreground">
            {value.language && <span>@{value.language}</span>}
            {value.datatype && (
              <span title={value.datatype}>^^{shortIri(value.datatype)}</span>
            )}
          </div>
        )}
      </div>
    );
  }

  return (
    <div className="rounded-md border border-dashed bg-muted/10 px-3 py-2.5">
      <div className="flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
        <Braces className="size-3.5" /> Blank node
      </div>
      <code className="mt-2 block break-all font-mono text-[10px] leading-5">
        _:{value.value}
      </code>
    </div>
  );
}
