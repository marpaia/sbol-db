/**
 * Primary chrome for the lab. Holds the brand mark, the top-level
 * route navigation grouped into collapsible categories, and a
 * "Tools" group for the command palette. Renders inside a shadcn
 * `Sidebar` so it collapses to an icon rail on desktop and slides
 * in from the left on mobile.
 */

import {
  Activity,
  BookOpen,
  Boxes,
  ChevronRight,
  Command as CommandIcon,
  Database,
  Dna,
  FileText,
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

const NAV_GROUPS: NavGroup[] = [
  {
    label: "Data",
    icon: <Boxes className="text-sbol-rbs" />,
    items: [
      { to: "/documents", icon: <FileText />, label: "Documents" },
      { to: "/objects", icon: <Boxes />, label: "Objects" },
      { to: "/sequences", icon: <Dna />, label: "Sequences" },
      { to: "/ontologies", icon: <Library />, label: "Ontologies" },
    ],
  },
  {
    label: "Query",
    icon: <Search className="text-sbol-promoter" />,
    items: [
      { to: "/schema", icon: <Table2 />, label: "Schema" },
      { to: "/sparql", icon: <Network />, label: "SPARQL" },
      { to: "/sql", icon: <Database />, label: "SQL" },
    ],
  },
  {
    label: "Operations",
    icon: <Activity className="text-sbol-terminator" />,
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
                <BrandMark />
                <div className="grid flex-1 text-left text-sm leading-tight">
                  <span className="truncate font-semibold tracking-tight">
                    SBOL Data Lab
                  </span>
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
              <NavItem
                to="/"
                end
                icon={<Home className="text-primary" />}
                label="Overview"
              />
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
