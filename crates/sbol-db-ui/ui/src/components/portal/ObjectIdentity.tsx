import { Fingerprint } from "lucide-react";

import { ObjectSection } from "@/components/portal/ObjectSection";
import { Badge } from "@/components/ui/badge";
import type { PortalObjectDetails } from "@/features/registry/objects/api";
import { shortIri } from "@/features/registry/objects/format";

export function ObjectIdentity({ object }: { object: PortalObjectDetails }) {
  return (
    <ObjectSection
      id="identity"
      icon={Fingerprint}
      title="Identity and classification"
      description="Stable identifiers and the biological vocabulary terms attached to this object."
    >
      <dl className="grid gap-x-8 gap-y-5 sm:grid-cols-2">
        <Property label="Display ID" value={object.display_id} />
        <Property label="Version" value={object.version} />
        <Property label="SBOL class" value={object.object_type} mono />
        <Property
          label="Persistent identity"
          value={object.persistent_identity}
          mono
        />
        <Property label="Source graph" value={object.source_graph} mono />
        <Property
          label="Content fingerprint"
          value={object.content_fingerprint}
          mono
        />
      </dl>

      <VocabularyTerms label="Types" values={object.types} />
      <VocabularyTerms label="Roles" values={object.roles} />
    </ObjectSection>
  );
}

function VocabularyTerms({
  label,
  values,
}: {
  label: string;
  values: string[];
}) {
  return (
    <div className="mt-6 border-t pt-5">
      <div className="flex items-baseline justify-between gap-3">
        <h3 className="text-xs font-medium uppercase tracking-[0.12em] text-muted-foreground">
          {label}
        </h3>
        <span className="text-xs tabular-nums text-muted-foreground">
          {values.length}
        </span>
      </div>
      {values.length > 0 ? (
        <div className="mt-3 flex flex-wrap gap-2">
          {values.map((value) => {
            const href = safeWebIri(value);
            const term = (
              <Badge
                variant="secondary"
                className="max-w-full font-mono text-[10px]"
                title={value}
              >
                <span className="truncate">{shortIri(value)}</span>
              </Badge>
            );
            return href ? (
              <a
                key={value}
                href={href}
                target="_blank"
                rel="noopener noreferrer"
                className="rounded-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
              >
                {term}
              </a>
            ) : (
              <span key={value}>{term}</span>
            );
          })}
        </div>
      ) : (
        <p className="mt-2 text-sm text-muted-foreground">
          No {label.toLowerCase()} are asserted on this object.
        </p>
      )}
    </div>
  );
}

function Property({
  label,
  value,
  mono,
  className,
}: {
  label: string;
  value?: string | null;
  mono?: boolean;
  className?: string;
}) {
  return (
    <div className={className}>
      <dt className="text-xs font-medium text-muted-foreground">{label}</dt>
      <dd
        className={
          mono ? "mt-1 break-all font-mono text-xs leading-5" : "mt-1 text-sm"
        }
      >
        {value || (
          <span className="text-muted-foreground/60">Not provided</span>
        )}
      </dd>
    </div>
  );
}

function safeWebIri(value: string): string | null {
  try {
    const url = new URL(value);
    return url.protocol === "http:" || url.protocol === "https:"
      ? url.href
      : null;
  } catch {
    return null;
  }
}
