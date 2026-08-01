/**
 * Primary chrome for the lab. Holds the brand mark, the top-level
 * route navigation grouped into collapsible categories, and a
 * "Tools" group for the command palette. Renders inside a shadcn
 * `Sidebar` so it collapses to an icon rail on desktop and slides
 * in from the left on mobile.
 */

import {
  Activity,
  ArchiveRestore,
  BookOpen,
  Boxes,
  Building2,
  ChevronRight,
  Command as CommandIcon,
  Database,
  Dna,
  Gauge,
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
  Undo2,
  Users,
} from "lucide-react";
import { NavLink, useMatch } from "react-router-dom";

import { useBackendInfo } from "@/hooks/useBackendInfo";
import type { Capabilities } from "@/lib/api";
import { PRODUCT_NAME } from "@/lib/product";
import { adminPath } from "@/lib/routes";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
  SidebarRail,
  SidebarSeparator,
} from "@/components/ui/sidebar";
import { BrandMark } from "./BrandMark";
import { ThemeToggle } from "./ThemeToggle";

export interface AppSidebarProps {
  onOpenPalette: () => void;
}

interface NavLeaf {
  to: string;
  end?: boolean;
  icon: React.ReactNode;
  label: string;
}

interface NavGroup {
  label: string;
  icon: React.ReactNode;
  items: NavLeaf[];
}

function navGroups(capabilities?: Capabilities): NavGroup[] {
  const queryItems: NavLeaf[] = [
    { to: adminPath("/schema"), icon: <Table2 />, label: "Schema" },
    { to: adminPath("/sparql"), icon: <Network />, label: "SPARQL" },
  ];
  if (capabilities?.sql_console) {
    queryItems.push({
      to: adminPath("/sql"),
      icon: <Database />,
      label: "SQL",
    });
  }

  const operationsItems: NavLeaf[] = [
    {
      to: adminPath("/observability"),
      end: true,
      icon: <Gauge />,
      label: "Metrics",
    },
    {
      to: adminPath("/observability/jobs"),
      end: true,
      icon: <ListChecks />,
      label: "Jobs",
    },
  ];
  if (capabilities && capabilities.maintenance !== null) {
    operationsItems.push({
      to: adminPath("/observability/maintenance"),
      icon: <HardDrive />,
      label: "Maintenance",
    });
  }

  return [
    {
      label: "Data",
      icon: <Boxes className="text-sbol-rbs" />,
      items: [
        { to: adminPath("/import"), icon: <Import />, label: "Import" },
        { to: adminPath("/graphs"), icon: <Share2 />, label: "Graphs" },
        { to: adminPath("/objects"), icon: <Boxes />, label: "Objects" },
        { to: adminPath("/sequences"), icon: <Dna />, label: "Sequences" },
        {
          to: adminPath("/ontologies"),
          icon: <Library />,
          label: "Ontologies",
        },
      ],
    },
    {
      label: "Query",
      icon: <Search className="text-sbol-promoter" />,
      items: queryItems,
    },
    {
      label: "Operations",
      icon: <Activity className="text-sbol-terminator" />,
      items: operationsItems,
    },
    {
      label: "Administration",
      icon: <Building2 className="text-primary" />,
      items: [
        {
          to: adminPath("/settings/instance"),
          icon: <Building2 />,
          label: "Instance",
        },
        {
          to: adminPath("/settings/users"),
          icon: <Users />,
          label: "Users",
        },
        {
          to: adminPath("/settings/integrations"),
          icon: <Plug />,
          label: "Integrations",
        },
        {
          to: adminPath("/settings/edge"),
          icon: <ServerCog />,
          label: "Edge runtime",
        },
        {
          to: adminPath("/operations/search"),
          icon: <SearchCheck />,
          label: "Search indexes",
        },
        {
          to: adminPath("/operations/backup"),
          icon: <ArchiveRestore />,
          label: "Backup & restore",
        },
        {
          to: adminPath("/operations/audit"),
          icon: <ScrollText />,
          label: "Activity",
        },
      ],
    },
  ];
}

export function AppSidebar({ onOpenPalette }: AppSidebarProps) {
  const { data: info } = useBackendInfo();
  const groups = navGroups(info?.capabilities);
  return (
    <Sidebar collapsible="icon" variant="sidebar">
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton size="lg" asChild tooltip="Admin workspace">
              <NavLink to={adminPath()}>
                <BrandMark />
                <div className="grid flex-1 text-left text-sm leading-tight">
                  <span className="truncate font-semibold tracking-tight">
                    {PRODUCT_NAME}
                  </span>
                  <span className="truncate text-xs text-sidebar-foreground/60">
                    Admin workspace
                  </span>
                </div>
              </NavLink>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>

      <SidebarContent>
        <SidebarGroup>
          <SidebarGroupLabel>Workspace</SidebarGroupLabel>
          <SidebarGroupContent>
            <SidebarMenu>
              <NavItem
                to={adminPath()}
                end
                icon={<Home className="text-primary" />}
                label="Overview"
              />
              {groups.map((group) => (
                <CollapsibleNavGroup key={group.label} group={group} />
              ))}
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>
      </SidebarContent>

      <SidebarFooter>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton asChild tooltip="Return to registry">
              <NavLink to="/">
                <Undo2 />
                <span>Return to registry</span>
              </NavLink>
            </SidebarMenuButton>
          </SidebarMenuItem>
          <SidebarMenuItem>
            <SidebarMenuButton
              onClick={onOpenPalette}
              tooltip="Command palette (⌘K)"
            >
              <CommandIcon />
              <span>Command palette</span>
              <kbd className="ml-auto text-[10px] text-sidebar-foreground/50">
                ⌘K
              </kbd>
            </SidebarMenuButton>
          </SidebarMenuItem>
          <SidebarMenuItem>
            <SidebarMenuButton asChild tooltip="API docs">
              <a href="/docs" target="_blank" rel="noopener noreferrer">
                <BookOpen />
                <span>API docs</span>
              </a>
            </SidebarMenuButton>
          </SidebarMenuItem>
          <SidebarSeparator className="my-1" />
          <ThemeToggle />
        </SidebarMenu>
      </SidebarFooter>

      <SidebarRail />
    </Sidebar>
  );
}

const ACTIVE_STRIPE =
  "before:absolute before:left-0 before:top-1.5 before:bottom-1.5 before:w-[3px] before:rounded-r before:bg-primary before:opacity-0 before:transition-opacity data-[active=true]:before:opacity-100";

function NavItem({
  to,
  end,
  icon,
  label,
}: {
  to: string;
  end?: boolean;
  icon: React.ReactNode;
  label: string;
}) {
  const match = useMatch({ path: to, end: end ?? false });
  return (
    <SidebarMenuItem>
      <SidebarMenuButton
        asChild
        isActive={!!match}
        tooltip={label}
        className={ACTIVE_STRIPE}
      >
        <NavLink to={to} end={end}>
          {icon}
          <span>{label}</span>
        </NavLink>
      </SidebarMenuButton>
    </SidebarMenuItem>
  );
}

function CollapsibleNavGroup({ group }: { group: NavGroup }) {
  return (
    <Collapsible defaultOpen className="group/collapsible" asChild>
      <SidebarMenuItem>
        <CollapsibleTrigger asChild>
          <SidebarMenuButton tooltip={group.label}>
            {group.icon}
            <span>{group.label}</span>
            <ChevronRight className="ml-auto transition-transform duration-200 group-data-[state=open]/collapsible:rotate-90" />
          </SidebarMenuButton>
        </CollapsibleTrigger>
        <CollapsibleContent>
          <SidebarMenuSub>
            {group.items.map((item) => (
              <SubNavItem key={item.to} {...item} />
            ))}
          </SidebarMenuSub>
        </CollapsibleContent>
      </SidebarMenuItem>
    </Collapsible>
  );
}

function SubNavItem({ to, end, icon, label }: NavLeaf) {
  const match = useMatch({ path: to, end: end ?? false });
  return (
    <SidebarMenuSubItem className="relative">
      <SidebarMenuSubButton
        asChild
        isActive={!!match}
        className={ACTIVE_STRIPE}
      >
        <NavLink to={to} end={end}>
          {icon}
          <span>{label}</span>
        </NavLink>
      </SidebarMenuSubButton>
    </SidebarMenuSubItem>
  );
}
