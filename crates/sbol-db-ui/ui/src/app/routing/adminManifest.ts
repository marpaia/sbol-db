import {
  Activity,
  ArchiveRestore,
  Boxes,
  Building2,
  Database,
  Dna,
  Gauge,
  GitBranch,
  HardDrive,
  Home,
  Import,
  Library,
  ListChecks,
  Network,
  Plug,
  ScrollText,
  Search,
  SearchCheck,
  ServerCog,
  Share2,
  Table2,
  Users,
  type LucideIcon,
} from "lucide-react";

import type { Capabilities } from "../../lib/api.ts";
import { adminPath } from "../../lib/routes.ts";

export type AdminSectionId =
  | "data-model"
  | "query"
  | "operations"
  | "administration";

export type AdminDestinationId =
  | "overview"
  | "import"
  | "graphs"
  | "objects"
  | "object-lookup"
  | "neighborhood"
  | "sequences"
  | "ontologies"
  | "schema"
  | "sparql"
  | "sql"
  | "metrics"
  | "jobs"
  | "maintenance"
  | "instance"
  | "users"
  | "integrations"
  | "edge"
  | "search-indexes"
  | "backups"
  | "audit";

export interface AdminSectionDefinition {
  id: AdminSectionId;
  label: string;
  icon: LucideIcon;
  iconClassName: string;
}

export interface AdminDestination {
  id: AdminDestinationId;
  path: string;
  label: string;
  paletteLabel?: string;
  section?: AdminSectionId;
  icon: LucideIcon;
  sidebar?: boolean;
  palette?: "query" | "go-to";
  end?: boolean;
  available?: (capabilities: Capabilities | undefined) => boolean;
}

export const adminSections: AdminSectionDefinition[] = [
  {
    id: "data-model",
    label: "Data model",
    icon: Boxes,
    iconClassName: "text-sbol-rbs",
  },
  {
    id: "query",
    label: "Query",
    icon: Search,
    iconClassName: "text-sidebar-primary",
  },
  {
    id: "operations",
    label: "Operations",
    icon: Activity,
    iconClassName: "text-sbol-terminator",
  },
  {
    id: "administration",
    label: "Administration",
    icon: Building2,
    iconClassName: "text-sidebar-primary",
  },
];

export const adminDestinations: AdminDestination[] = [
  {
    id: "overview",
    path: adminPath(),
    label: "Overview",
    icon: Home,
    sidebar: true,
    palette: "go-to",
    end: true,
  },
  {
    id: "import",
    path: adminPath("/import"),
    label: "Import",
    section: "data-model",
    icon: Import,
    sidebar: true,
    palette: "go-to",
  },
  {
    id: "graphs",
    path: adminPath("/graphs"),
    label: "Graphs",
    section: "data-model",
    icon: Share2,
    sidebar: true,
    palette: "go-to",
  },
  {
    id: "objects",
    path: adminPath("/objects"),
    label: "Objects",
    section: "data-model",
    icon: Boxes,
    sidebar: true,
    palette: "go-to",
  },
  {
    id: "object-lookup",
    path: adminPath("/objects/lookup"),
    label: "Bulk object lookup",
    section: "data-model",
    icon: Boxes,
    palette: "go-to",
  },
  {
    id: "neighborhood",
    path: adminPath("/neighborhood"),
    label: "Neighborhood",
    paletteLabel: "Walk neighborhood",
    section: "data-model",
    icon: GitBranch,
    palette: "go-to",
  },
  {
    id: "sequences",
    path: adminPath("/sequences"),
    label: "Sequences",
    paletteLabel: "Sequence search",
    section: "data-model",
    icon: Dna,
    sidebar: true,
    palette: "go-to",
  },
  {
    id: "ontologies",
    path: adminPath("/ontologies"),
    label: "Ontologies",
    section: "data-model",
    icon: Library,
    sidebar: true,
    palette: "go-to",
  },
  {
    id: "schema",
    path: adminPath("/schema"),
    label: "Schema",
    section: "query",
    icon: Table2,
    sidebar: true,
    palette: "go-to",
  },
  {
    id: "sparql",
    path: adminPath("/sparql"),
    label: "SPARQL",
    section: "query",
    icon: Network,
    sidebar: true,
    palette: "query",
  },
  {
    id: "sql",
    path: adminPath("/sql"),
    label: "SQL",
    section: "query",
    icon: Database,
    sidebar: true,
    palette: "query",
    available: (capabilities) => Boolean(capabilities?.sql_console),
  },
  {
    id: "metrics",
    path: adminPath("/observability"),
    label: "Metrics",
    section: "operations",
    icon: Gauge,
    sidebar: true,
    palette: "go-to",
    end: true,
  },
  {
    id: "jobs",
    path: adminPath("/observability/jobs"),
    label: "Jobs",
    section: "operations",
    icon: ListChecks,
    sidebar: true,
    end: true,
  },
  {
    id: "maintenance",
    path: adminPath("/observability/maintenance"),
    label: "Maintenance",
    section: "operations",
    icon: HardDrive,
    sidebar: true,
    palette: "go-to",
    available: (capabilities) =>
      Boolean(capabilities && capabilities.maintenance !== null),
  },
  {
    id: "instance",
    path: adminPath("/settings/instance"),
    label: "Instance",
    section: "administration",
    icon: Building2,
    sidebar: true,
  },
  {
    id: "users",
    path: adminPath("/settings/users"),
    label: "Users",
    section: "administration",
    icon: Users,
    sidebar: true,
  },
  {
    id: "integrations",
    path: adminPath("/settings/integrations"),
    label: "Integrations",
    section: "administration",
    icon: Plug,
    sidebar: true,
  },
  {
    id: "edge",
    path: adminPath("/settings/edge"),
    label: "Edge runtime",
    section: "administration",
    icon: ServerCog,
    sidebar: true,
  },
  {
    id: "search-indexes",
    path: adminPath("/operations/search"),
    label: "Search indexes",
    section: "administration",
    icon: SearchCheck,
    sidebar: true,
  },
  {
    id: "backups",
    path: adminPath("/operations/backup"),
    label: "Backups & recovery",
    section: "administration",
    icon: ArchiveRestore,
    sidebar: true,
  },
  {
    id: "audit",
    path: adminPath("/operations/audit"),
    label: "Activity",
    section: "administration",
    icon: ScrollText,
    sidebar: true,
  },
];

export function adminDestination(id: AdminDestinationId): AdminDestination {
  const destination = adminDestinations.find((item) => item.id === id);
  if (!destination) throw new Error(`Unknown admin destination: ${id}`);
  return destination;
}

/** Relative path consumed by the route tree mounted at `/admin/*`. */
export function adminRouteSegment(id: AdminDestinationId): string {
  return adminDestination(id).path.replace(/^\/admin\/?/, "");
}

export function availableAdminDestinations(
  capabilities: Capabilities | undefined
): AdminDestination[] {
  return adminDestinations.filter(
    (destination) =>
      destination.available === undefined || destination.available(capabilities)
  );
}

export type AdminCrumb = { label: string; to?: string; mono?: boolean };

export function adminBreadcrumbs(pathname: string): AdminCrumb[] {
  const objectLookup = adminDestination("object-lookup");
  if (pathname === objectLookup.path) {
    return sectionTrail("data-model", adminDestination("objects"), {
      label: "Bulk lookup",
    });
  }

  const ontologyMatch = pathname.match(/^\/admin\/ontologies\/([^/]+)\/?$/);
  if (ontologyMatch) {
    return sectionTrail("data-model", adminDestination("ontologies"), {
      label: decodeURIComponent(ontologyMatch[1]).toLowerCase(),
      mono: true,
    });
  }

  const tableMatch = pathname.match(/^\/admin\/schema\/tables\/([^/]+)\/?$/);
  if (tableMatch) {
    return sectionTrail("query", adminDestination("schema"), {
      label: decodeURIComponent(tableMatch[1]),
      mono: true,
    });
  }

  const graphMatch = pathname.match(/^\/admin\/graphs\/([^/]+)\/?$/);
  if (graphMatch) {
    return sectionTrail("data-model", adminDestination("graphs"), {
      label: shortId(decodeURIComponent(graphMatch[1])),
      mono: true,
    });
  }

  const jobMatch = pathname.match(/^\/admin\/observability\/jobs\/([^/]+)\/?$/);
  if (jobMatch) {
    return sectionTrail("operations", adminDestination("jobs"), {
      label: shortId(decodeURIComponent(jobMatch[1])),
      mono: true,
    });
  }

  const objectMatch = pathname.match(/^\/admin\/objects\/([^/]+)\/?$/);
  if (objectMatch) {
    return sectionTrail("data-model", adminDestination("objects"), {
      label: shortLabel(decodeURIComponent(objectMatch[1])),
      mono: true,
    });
  }

  const candidates = adminDestinations
    .filter((destination) => destination.section)
    .sort((a, b) => b.path.length - a.path.length);
  const destination = candidates.find(
    (candidate) =>
      pathname === candidate.path || pathname.startsWith(`${candidate.path}/`)
  );
  if (!destination || !destination.section) return [{ label: "Overview" }];

  return sectionTrail(destination.section, destination);
}

function sectionTrail(
  sectionId: AdminSectionId,
  destination: AdminDestination,
  detail?: AdminCrumb
): AdminCrumb[] {
  const section = adminSections.find((item) => item.id === sectionId);
  const trail: AdminCrumb[] = [
    { label: section?.label ?? sectionId },
    { label: destination.label, to: destination.path },
  ];
  if (detail) trail.push(detail);
  return trail;
}

function shortId(id: string): string {
  if (id.length <= 12) return id;
  return `${id.slice(0, 8)}…`;
}

function shortLabel(iri: string): string {
  const match = iri.match(/[#/]([^#/]+)$/);
  return match ? match[1] : iri.length > 32 ? `${iri.slice(0, 32)}…` : iri;
}
