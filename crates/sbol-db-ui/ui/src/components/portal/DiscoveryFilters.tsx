import { useEffect, useState } from "react";
import { ChevronDown, RotateCcw, SlidersHorizontal, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import { NativeSelect } from "@/components/ui/native-select";
import type {
  DiscoveryFacets,
  DiscoveryFacetValue,
  PortalSearchQuery,
} from "@/features/registry/discovery/api";
import { shortIri } from "@/features/registry/objects/format";

export type DiscoveryFilterKey =
  | "q"
  | "type"
  | "role"
  | "collection"
  | "owner"
  | "provenance"
  | "createdAfter"
  | "createdBefore"
  | "modifiedAfter"
  | "modifiedBefore";

type FilterChanges = Partial<Record<DiscoveryFilterKey, string | undefined>>;

interface DiscoveryFiltersProps {
  query: PortalSearchQuery;
  facets?: DiscoveryFacets;
  facetsLoading?: boolean;
  facetsError?: string;
  onChange: (changes: FilterChanges) => void;
  onClear: () => void;
  onApplied?: () => void;
}

interface AdvancedDraft {
  type: string;
  role: string;
  collection: string;
  owner: string;
  provenance: string;
  createdAfter: string;
  createdBefore: string;
  modifiedAfter: string;
  modifiedBefore: string;
}

export function DiscoveryFilters({
  query,
  facets,
  facetsLoading,
  facetsError,
  onChange,
  onClear,
  onApplied,
}: DiscoveryFiltersProps) {
  const [advanced, setAdvanced] = useState<AdvancedDraft>(() =>
    advancedFrom(query)
  );

  useEffect(() => {
    setAdvanced(advancedFrom(query));
  }, [query]);

  const hasAdvanced = Object.values(advancedFrom(query)).some(Boolean);
  const applyAdvanced = () => {
    onChange({
      type: optional(advanced.type),
      role: optional(advanced.role),
      collection: optional(advanced.collection),
      owner: optional(advanced.owner),
      provenance: optional(advanced.provenance),
      createdAfter: optional(advanced.createdAfter),
      createdBefore: optional(advanced.createdBefore),
      modifiedAfter: optional(advanced.modifiedAfter),
      modifiedBefore: optional(advanced.modifiedBefore),
    });
    onApplied?.();
  };

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <SlidersHorizontal className="size-4 text-primary" />
          <h2 className="text-sm font-semibold">Refine results</h2>
        </div>
        <Button variant="ghost" size="sm" onClick={onClear}>
          <RotateCcw /> Reset
        </Button>
      </div>

      <div className="space-y-4">
        <FacetSelect
          id="discovery-type"
          label="Object type"
          emptyLabel="All object types"
          value={query.type || ""}
          values={facets?.types || []}
          loading={facetsLoading}
          onChange={(value) => onChange({ type: optional(value) })}
        />
        <FacetSelect
          id="discovery-role"
          label="Biological role"
          emptyLabel="All biological roles"
          value={query.role || ""}
          values={facets?.roles || []}
          loading={facetsLoading}
          onChange={(value) => onChange({ role: optional(value) })}
        />
        {facetsError && (
          <p className="text-xs leading-5 text-destructive">
            Facet labels are unavailable. Existing URL filters still apply.
          </p>
        )}
      </div>

      <Collapsible defaultOpen={hasAdvanced}>
        <CollapsibleTrigger asChild>
          <Button
            variant="ghost"
            className="group -mx-2 w-[calc(100%+1rem)] justify-between"
          >
            Advanced filters
            {hasAdvanced && (
              <span className="ml-auto rounded-full bg-primary/10 px-2 py-0.5 text-[10px] font-semibold text-primary">
                Active
              </span>
            )}
            <ChevronDown className="transition-transform duration-150 group-data-[state=open]:rotate-180" />
          </Button>
        </CollapsibleTrigger>
        <CollapsibleContent className="pt-4">
          <form
            className="space-y-4"
            onSubmit={(event) => {
              event.preventDefault();
              applyAdvanced();
            }}
          >
            <TextFilter
              id="discovery-exact-type"
              label="Exact type IRI"
              placeholder="http://sbols.org/v3#Component"
              value={advanced.type}
              onChange={(type) =>
                setAdvanced((current) => ({ ...current, type }))
              }
            />
            <TextFilter
              id="discovery-exact-role"
              label="Exact role IRI"
              placeholder="http://identifiers.org/so/SO:…"
              value={advanced.role}
              onChange={(role) =>
                setAdvanced((current) => ({ ...current, role }))
              }
            />
            <TextFilter
              id="discovery-collection"
              label="Collection IRI"
              placeholder="https://…/collection"
              value={advanced.collection}
              onChange={(collection) =>
                setAdvanced((current) => ({ ...current, collection }))
              }
            />
            <TextFilter
              id="discovery-owner"
              label="Owner IRI"
              placeholder="https://…/user/graph"
              value={advanced.owner}
              onChange={(owner) =>
                setAdvanced((current) => ({ ...current, owner }))
              }
            />
            <TextFilter
              id="discovery-provenance"
              label="Provenance contains"
              placeholder="Source or attribution"
              value={advanced.provenance}
              onChange={(provenance) =>
                setAdvanced((current) => ({ ...current, provenance }))
              }
            />

            <DateRange
              legend="Created"
              prefix="created"
              after={advanced.createdAfter}
              before={advanced.createdBefore}
              onAfter={(createdAfter) =>
                setAdvanced((current) => ({ ...current, createdAfter }))
              }
              onBefore={(createdBefore) =>
                setAdvanced((current) => ({ ...current, createdBefore }))
              }
            />
            <DateRange
              legend="Modified"
              prefix="modified"
              after={advanced.modifiedAfter}
              before={advanced.modifiedBefore}
              onAfter={(modifiedAfter) =>
                setAdvanced((current) => ({ ...current, modifiedAfter }))
              }
              onBefore={(modifiedBefore) =>
                setAdvanced((current) => ({ ...current, modifiedBefore }))
              }
            />

            <Button type="submit" className="w-full">
              Apply advanced filters
            </Button>
          </form>
        </CollapsibleContent>
      </Collapsible>
    </div>
  );
}

export function DiscoveryFilterSummary({
  query,
  facets,
  onRemove,
}: {
  query: PortalSearchQuery;
  facets?: DiscoveryFacets;
  onRemove: (key: DiscoveryFilterKey) => void;
}) {
  const filters: Array<{
    key: DiscoveryFilterKey;
    label: string;
    value: string | undefined;
  }> = [
    { key: "q", label: "Text", value: query.q ? `“${query.q}”` : undefined },
    {
      key: "type",
      label: "Type",
      value: facetLabel(facets?.types, query.type),
    },
    {
      key: "role",
      label: "Role",
      value: facetLabel(facets?.roles, query.role),
    },
    {
      key: "collection",
      label: "Collection",
      value: query.collection ? shortIri(query.collection) : undefined,
    },
    {
      key: "owner",
      label: "Owner",
      value: query.owner ? shortIri(query.owner) : undefined,
    },
    {
      key: "provenance",
      label: "Provenance",
      value: query.provenance,
    },
    { key: "createdAfter", label: "Created after", value: query.createdAfter },
    {
      key: "createdBefore",
      label: "Created before",
      value: query.createdBefore,
    },
    {
      key: "modifiedAfter",
      label: "Modified after",
      value: query.modifiedAfter,
    },
    {
      key: "modifiedBefore",
      label: "Modified before",
      value: query.modifiedBefore,
    },
  ];
  const active = filters.filter(
    (filter): filter is typeof filter & { value: string } =>
      Boolean(filter.value)
  );
  if (active.length === 0) return null;

  return (
    <div className="flex flex-wrap gap-2" aria-label="Active search filters">
      {active.map((filter) => (
        <button
          key={filter.key}
          type="button"
          onClick={() => onRemove(filter.key)}
          className="inline-flex h-7 max-w-full items-center gap-1.5 rounded-full border bg-background px-2.5 text-xs text-muted-foreground transition-[color,border-color,background-color] duration-150 hover:border-primary/35 hover:bg-primary/5 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 active:bg-primary/10"
          aria-label={`Remove ${filter.label.toLowerCase()} filter ${filter.value}`}
        >
          <span className="font-medium text-foreground">{filter.label}</span>
          <span className="max-w-48 truncate">{filter.value}</span>
          <X className="size-3" />
        </button>
      ))}
    </div>
  );
}

function FacetSelect({
  id,
  label,
  emptyLabel,
  value,
  values,
  loading,
  onChange,
}: {
  id: string;
  label: string;
  emptyLabel: string;
  value: string;
  values: DiscoveryFacetValue[];
  loading?: boolean;
  onChange: (value: string) => void;
}) {
  const selectedIsMissing =
    value && !values.some((facet) => facet.iri === value);
  return (
    <div className="space-y-1.5">
      <label htmlFor={id} className="text-xs font-medium">
        {label}
      </label>
      <NativeSelect
        id={id}
        value={value}
        disabled={loading}
        onChange={(event) => onChange(event.target.value)}
      >
        <option value="">{loading ? "Loading facets…" : emptyLabel}</option>
        {selectedIsMissing && <option value={value}>{shortIri(value)}</option>}
        {values.map((facet) => (
          <option key={facet.iri} value={facet.iri}>
            {facet.label}
            {facet.curie && facet.curie !== facet.label
              ? ` · ${facet.curie}`
              : ""}{" "}
            ({facet.count.toLocaleString()})
          </option>
        ))}
      </NativeSelect>
    </div>
  );
}

function TextFilter({
  id,
  label,
  placeholder,
  value,
  onChange,
}: {
  id: string;
  label: string;
  placeholder: string;
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <div className="space-y-1.5">
      <label htmlFor={id} className="text-xs font-medium">
        {label}
      </label>
      <Input
        id={id}
        value={value}
        placeholder={placeholder}
        onChange={(event) => onChange(event.target.value)}
        className="font-mono text-xs"
      />
    </div>
  );
}

function DateRange({
  legend,
  prefix,
  after,
  before,
  onAfter,
  onBefore,
}: {
  legend: string;
  prefix: string;
  after: string;
  before: string;
  onAfter: (value: string) => void;
  onBefore: (value: string) => void;
}) {
  return (
    <fieldset className="space-y-2">
      <legend className="text-xs font-medium">{legend}</legend>
      <div className="grid grid-cols-2 gap-2">
        <div className="space-y-1">
          <label
            htmlFor={`${prefix}-after`}
            className="text-[10px] uppercase tracking-wide text-muted-foreground"
          >
            After
          </label>
          <Input
            id={`${prefix}-after`}
            type="date"
            value={after}
            max={before || undefined}
            onChange={(event) => onAfter(event.target.value)}
            className="px-2 text-xs"
          />
        </div>
        <div className="space-y-1">
          <label
            htmlFor={`${prefix}-before`}
            className="text-[10px] uppercase tracking-wide text-muted-foreground"
          >
            Before
          </label>
          <Input
            id={`${prefix}-before`}
            type="date"
            value={before}
            min={after || undefined}
            onChange={(event) => onBefore(event.target.value)}
            className="px-2 text-xs"
          />
        </div>
      </div>
    </fieldset>
  );
}

function advancedFrom(query: PortalSearchQuery): AdvancedDraft {
  return {
    type: query.type || "",
    role: query.role || "",
    collection: query.collection || "",
    owner: query.owner || "",
    provenance: query.provenance || "",
    createdAfter: query.createdAfter || "",
    createdBefore: query.createdBefore || "",
    modifiedAfter: query.modifiedAfter || "",
    modifiedBefore: query.modifiedBefore || "",
  };
}

function optional(value: string): string | undefined {
  return value.trim() || undefined;
}

function facetLabel(
  values: DiscoveryFacetValue[] | undefined,
  iri: string | undefined
): string | undefined {
  if (!iri) return undefined;
  const facet = values?.find((candidate) => candidate.iri === iri);
  return facet?.label || facet?.curie || shortIri(iri);
}
