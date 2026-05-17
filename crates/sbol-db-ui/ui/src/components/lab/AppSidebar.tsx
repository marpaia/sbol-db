/**
 * Primary chrome for the lab. Holds the brand mark, the top-level
 * route navigation grouped into collapsible categories, and a
 * "Tools" group for the command palette. Renders inside a shadcn
 * `Sidebar` so it collapses to an icon rail on desktop and slides
 * in from the left on mobile.
 */

import {
  Activity,
  ChevronRight,
  Command as CommandIcon,
  Compass,
  Database,
  FlaskConical,
  Gauge,
  HardDrive,
  Home,
  Library,
  Network,
  Search,
  Table2,
} from "lucide-react";
import { NavLink, useMatch } from "react-router-dom";

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

const NAV_GROUPS: NavGroup[] = [
  {
    label: "Query",
    icon: <Search />,
    items: [
      { to: "/sparql", icon: <Network />, label: "SPARQL" },
      { to: "/sql", icon: <Database />, label: "SQL" },
    ],
  },
  {
    label: "Explore",
    icon: <Compass />,
    items: [
      { to: "/schema", icon: <Table2 />, label: "Schema" },
      { to: "/ontologies", icon: <Library />, label: "Ontologies" },
    ],
  },
  {
    label: "Observability",
    icon: <Activity />,
    items: [
      { to: "/observability", end: true, icon: <Gauge />, label: "Metrics" },
      {
        to: "/observability/postgres",
        icon: <HardDrive />,
        label: "Postgres",
      },
    ],
  },
];

export function AppSidebar({ onOpenPalette }: AppSidebarProps) {
  return (
    <Sidebar collapsible="icon" variant="sidebar">
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton size="lg" asChild tooltip="SBOL Data Lab">
              <NavLink to="/">
                <div className="flex aspect-square size-8 items-center justify-center rounded-lg bg-primary/15 text-primary">
                  <FlaskConical className="size-4" />
                </div>
                <div className="grid flex-1 text-left text-sm leading-tight">
                  <span className="truncate font-semibold">SBOL Data Lab</span>
                  <span className="truncate text-xs text-sidebar-foreground/60">
                    Powered by sbol-db 🦀
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
              <NavItem to="/" end icon={<Home />} label="Overview" />
              {NAV_GROUPS.map((group) => (
                <CollapsibleNavGroup key={group.label} group={group} />
              ))}
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>
      </SidebarContent>

      <SidebarFooter>
        <SidebarMenu>
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
          <SidebarSeparator className="my-1" />
          <ThemeToggle />
        </SidebarMenu>
      </SidebarFooter>

      <SidebarRail />
    </Sidebar>
  );
}

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
      <SidebarMenuButton asChild isActive={!!match} tooltip={label}>
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
    <SidebarMenuSubItem>
      <SidebarMenuSubButton asChild isActive={!!match}>
        <NavLink to={to} end={end}>
          {icon}
          <span>{label}</span>
        </NavLink>
      </SidebarMenuSubButton>
    </SidebarMenuSubItem>
  );
}
