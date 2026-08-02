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
  Share2,
  Table2,
  Users,
} from "lucide-react";
import { NavLink, useMatch } from "react-router-dom";

import { useBackendInfo } from "@/hooks/useBackendInfo";
import type { Capabilities } from "@/lib/api";
import { ProductModeSwitch } from "@/components/product/ProductModeSwitch";
import { useInstance } from "@/features/portal/queries";
import { deploymentName, PRODUCT_NAME } from "@/lib/product";
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
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
} from "@/components/ui/sidebar";
import { BrandMark } from "./BrandMark";

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
      label: "Data model",
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
      icon: <Search className="text-sidebar-primary" />,
      items: queryItems,
    },
    {
      label: "Operations",
      icon: <Activity className="text-sbol-terminator" />,
      items: operationsItems,
    },
    {
      label: "Administration",
      icon: <Building2 className="text-sidebar-primary" />,
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
  const instance = useInstance();
  const groups = navGroups(info?.capabilities);
  const deployment = deploymentName(instance.data?.name);
  return (
    <Sidebar
      collapsible="icon"
      variant="sidebar"
      className="border-sidebar-border pt-1"
    >
      <SidebarHeader className="h-[4.25rem] justify-center border-b border-sidebar-border px-2 py-0">
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton
              size="lg"
              asChild
              tooltip="Admin workspace"
              className="rounded-[3px] hover:bg-sidebar-accent/60"
            >
              <NavLink to={adminPath()}>
                <BrandMark className="[&_svg]:!stroke-sidebar-primary" />
                <div className="grid flex-1 text-left text-sm leading-tight">
                  <span className="truncate font-mono text-xs font-semibold tracking-[0.08em] text-sidebar-accent-foreground">
                    {PRODUCT_NAME}
                  </span>
                  <span className="truncate font-mono text-[9px] uppercase tracking-[0.12em] text-sidebar-foreground/55">
                    {deployment || "Admin control plane"}
                  </span>
                </div>
              </NavLink>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>

      <div className="border-b border-sidebar-border p-2 group-data-[collapsible=icon]:hidden">
        <ProductModeSwitch mode="admin" />
      </div>

      <SidebarContent className="py-1">
        <SidebarGroup className="px-2 py-2">
          <SidebarGroupContent>
            <SidebarMenu className="gap-0.5">
              <NavItem
                to={adminPath()}
                end
                icon={<Home className="text-sidebar-foreground/55" />}
                label="Overview"
              />
              {groups.map((group) => (
                <CollapsibleNavGroup key={group.label} group={group} />
              ))}
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>
      </SidebarContent>

      <SidebarFooter className="border-t border-sidebar-border p-2">
        <SidebarMenu className="gap-0.5">
          <SidebarMenuItem>
            <SidebarMenuButton
              onClick={onOpenPalette}
              tooltip="Command palette (⌘K)"
              className="rounded-[3px] text-xs"
            >
              <CommandIcon />
              <span>Command palette</span>
              <kbd className="ml-auto text-[10px] text-sidebar-foreground/50">
                ⌘K
              </kbd>
            </SidebarMenuButton>
          </SidebarMenuItem>
          <SidebarMenuItem>
            <SidebarMenuButton
              asChild
              tooltip="API reference"
              className="rounded-[3px] text-xs"
            >
              <a href="/api/v2/docs" target="_blank" rel="noopener noreferrer">
                <BookOpen />
                <span>API reference</span>
              </a>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarFooter>
    </Sidebar>
  );
}

const ACTIVE_STRIPE =
  "rounded-[3px] before:absolute before:left-0 before:top-1.5 before:bottom-1.5 before:w-[2px] before:bg-sidebar-primary before:opacity-0 before:transition-opacity data-[active=true]:before:opacity-100";

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
        className={`${ACTIVE_STRIPE} text-xs text-sidebar-foreground/70 data-[active=true]:bg-sidebar-accent/40 data-[active=true]:font-normal`}
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
          <SidebarMenuButton
            tooltip={group.label}
            className="rounded-[3px] font-mono text-[10px] uppercase tracking-[0.08em] text-sidebar-foreground/65 data-[state=open]:bg-sidebar-accent/35 data-[state=open]:text-sidebar-accent-foreground"
          >
            {group.icon}
            <span>{group.label}</span>
            <ChevronRight className="ml-auto size-3.5 transition-transform duration-150 [transition-timing-function:cubic-bezier(0.23,1,0.32,1)] group-data-[state=open]/collapsible:rotate-90" />
          </SidebarMenuButton>
        </CollapsibleTrigger>
        <CollapsibleContent>
          <SidebarMenuSub className="mx-4 gap-0.5 border-sidebar-border/80 px-2 py-1">
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
        className={`${ACTIVE_STRIPE} h-8 text-xs text-sidebar-foreground/70`}
      >
        <NavLink to={to} end={end}>
          {icon}
          <span>{label}</span>
        </NavLink>
      </SidebarMenuSubButton>
    </SidebarMenuSubItem>
  );
}
