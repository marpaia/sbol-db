import type { DiscoverySort, PortalSearchQuery, SortDirection } from "./api";

export const DEFAULT_DISCOVERY_PAGE_SIZE = 24;
export const DISCOVERY_PAGE_SIZES = [12, 24, 48, 100] as const;

export type DiscoveryView = "grid" | "list";

export interface DiscoveryRouteState {
  query: PortalSearchQuery & {
    offset: number;
    limit: number;
    sort: DiscoverySort;
    direction: SortDirection;
  };
  view: DiscoveryView;
  compatibilityWarnings: string[];
  translatedFromClassic: boolean;
}

export interface ClassicSearchLocation {
  pathname: "/search";
  params: URLSearchParams;
}

export function canonicalSequenceSearchParams(
  params: URLSearchParams
): URLSearchParams {
  const canonical = new URLSearchParams(params);
  canonical.set("kind", "sequence");
  return canonical;
}

const SORTS = new Set<DiscoverySort>([
  "relevance",
  "name",
  "created",
  "modified",
  "iri",
]);

export function parseDiscoveryParams(
  params: URLSearchParams
): DiscoveryRouteState {
  const sortValue = params.get("sort") as DiscoverySort | null;
  const sort = sortValue && SORTS.has(sortValue) ? sortValue : "relevance";
  const directionValue = params.get("direction");
  const direction =
    directionValue === "asc" || directionValue === "desc"
      ? directionValue
      : naturalDirection(sort);

  return {
    query: {
      q: optional(params.get("q")),
      type: optional(params.get("type")),
      role: optional(params.get("role")),
      collection: optional(params.get("collection")),
      owner: optional(params.get("owner")),
      provenance: optional(params.get("provenance")),
      createdAfter: date(params.get("created_after")),
      createdBefore: date(params.get("created_before")),
      modifiedAfter: date(params.get("modified_after")),
      modifiedBefore: date(params.get("modified_before")),
      sort,
      direction,
      offset: nonNegativeInteger(params.get("offset"), 0),
      limit: boundedInteger(
        params.get("limit"),
        DEFAULT_DISCOVERY_PAGE_SIZE,
        1,
        1000
      ),
    },
    view: params.get("view") === "list" ? "list" : "grid",
    compatibilityWarnings: params
      .getAll("compat_warning")
      .map((warning) => warning.trim())
      .filter(Boolean),
    translatedFromClassic: params.get("compat") === "classic",
  };
}

export function naturalDirection(sort: DiscoverySort): SortDirection {
  return sort === "name" || sort === "iri" ? "asc" : "desc";
}

/**
 * Translate the supported subset of the classic path grammar into the native
 * discovery URL. Unsupported predicates remain explicit warnings instead of
 * silently broadening the query and pretending the filter was applied.
 */
export function translateClassicSearchPath(
  path: string
): ClassicSearchLocation {
  const params = new URLSearchParams();
  params.set("compat", "classic");
  const decoded = safeDecode(path).replace(/^\/+|\/+$/g, "");
  const parts = decoded.split("&").filter(Boolean);
  const sequenceFacet = parts.find((part) => {
    const key = part.slice(0, part.indexOf("="));
    return ["sequence", "globalsequence", "exactsequence"].includes(key);
  });

  if (sequenceFacet) {
    const separator = sequenceFacet.indexOf("=");
    const key = sequenceFacet.slice(0, separator);
    const value = stripAngles(sequenceFacet.slice(separator + 1).trim());
    if (value) params.set("q", value);
    params.set("kind", "sequence");
    params.set("mode", key === "exactsequence" ? "exact" : "global");
    for (const part of parts) {
      if (part === sequenceFacet) continue;
      warn(
        params,
        `The classic sequence handler ignores the additional segment “${part}”.`
      );
    }
    return { pathname: "/search", params };
  }

  parts.forEach((part, index) => {
    if (!part.includes("=")) {
      if (index === parts.length - 1) {
        const text = part
          .split(/\s+/)
          .filter((token) => !["and", "or", "not"].includes(token))
          .join(" ")
          .trim();
        if (text) params.set("q", text);
      } else {
        warn(params, `The classic search segment “${part}” is malformed.`);
      }
      return;
    }

    const separator = part.indexOf("=");
    const key = part.slice(0, separator).trim();
    const value = stripAngles(part.slice(separator + 1).trim());
    if (!key || !value) {
      warn(params, `The classic filter “${part}” has no usable value.`);
      return;
    }

    switch (key) {
      case "objectType": {
        const type = expandObjectType(value);
        if (type) params.set("type", type);
        else warn(params, `The object type “${value}” uses an unknown prefix.`);
        break;
      }
      case "collection":
        params.set("collection", value);
        break;
      case "createdAfter":
        params.set("created_after", value);
        break;
      case "createdBefore":
        params.set("created_before", value);
        break;
      case "modifiedAfter":
        params.set("modified_after", value);
        break;
      case "modifiedBefore":
        params.set("modified_before", value);
        break;
      case "role":
      case "sbol2:role":
      case "sbol3:role":
        params.set("role", value);
        break;
      case "ownedBy":
      case "sbh:ownedBy":
        params.set("owner", value);
        break;
      case "mutableProvenance":
      case "sbh:mutableProvenance":
        params.set("provenance", value);
        break;
      case "sequence":
      case "globalsequence":
      case "exactsequence":
        warn(
          params,
          "Sequence-search links are not yet represented in native discovery."
        );
        break;
      default:
        warn(
          params,
          `The classic predicate “${key}” is not available as a native discovery filter.`
        );
    }
  });

  return { pathname: "/search", params };
}

export function hasDiscoveryFilters(query: PortalSearchQuery): boolean {
  return Boolean(
    query.q ||
    query.type ||
    query.role ||
    query.collection ||
    query.owner ||
    query.provenance ||
    query.createdAfter ||
    query.createdBefore ||
    query.modifiedAfter ||
    query.modifiedBefore
  );
}

function warn(params: URLSearchParams, message: string) {
  params.append("compat_warning", message);
}

function expandObjectType(value: string): string | null {
  if (value.startsWith("http://") || value.startsWith("https://")) return value;
  if (value.startsWith("sbol2:")) {
    return `http://sbols.org/v2#${value.slice("sbol2:".length)}`;
  }
  if (value.startsWith("sbol3:")) {
    return `http://sbols.org/v3#${value.slice("sbol3:".length)}`;
  }
  if (value.includes(":")) return null;
  return `http://sbols.org/v2#${value}`;
}

function stripAngles(value: string): string {
  return value.startsWith("<") && value.endsWith(">")
    ? value.slice(1, -1)
    : value;
}

function safeDecode(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

function optional(value: string | null): string | undefined {
  const normalized = value?.trim();
  return normalized || undefined;
}

function date(value: string | null): string | undefined {
  return value && /^\d{4}-\d{2}-\d{2}$/.test(value) ? value : undefined;
}

function nonNegativeInteger(value: string | null, fallback: number): number {
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed >= 0 ? parsed : fallback;
}

function boundedInteger(
  value: string | null,
  fallback: number,
  minimum: number,
  maximum: number
): number {
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed >= minimum && parsed <= maximum
    ? parsed
    : fallback;
}
