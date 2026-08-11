/**
 * Primary chrome for the lab. Holds the brand mark, the top-level
 * route navigation grouped into collapsible categories, and a
 * "Tools" group for the command palette. Renders inside a shadcn
 * `Sidebar` so it collapses to an icon rail on desktop and slides
 * in from the left on mobile.
 */

import { BookOpen, ChevronRight, Command as CommandIcon } from "lucide-react";
import { NavLink, useMatch } from "react-router-dom";

import {
  adminDestination,
  adminSections,
  availableAdminDestinations,
  type AdminDestination,
} from "@/app/routing/adminManifest";
import { useBackendInfo } from "@/features/admin/backend/queries";
import { ProductModeSwitch } from "@/components/product/ProductModeSwitch";
import { useInstance } from "@/features/instance/queries";
import { deploymentName, PRODUCT_NAME } from "@/lib/product";
import { adminPath, API_DOCS_PATH } from "@/lib/routes";
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

export function AppSidebar({ onOpenPalette }: AppSidebarProps) {
  const { data: info } = useBackendInfo();
  const instance = useInstance();
  const destinations = availableAdminDestinations(info);
  const overview = adminDestination("overview");
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
                destination={overview}
                iconClassName="text-sidebar-foreground/55"
              />
              {adminSections.map((section) => {
                const items = destinations.filter(
                  (destination) =>
                    destination.sidebar && destination.section === section.id
                );
                return items.length ? (
                  <CollapsibleNavGroup
                    key={section.id}
                    label={section.label}
                    icon={<section.icon className={section.iconClassName} />}
                    items={items}
                  />
                ) : null;
              })}
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
              <a href={API_DOCS_PATH} target="_blank" rel="noopener noreferrer">
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
  destination,
  iconClassName,
}: {
  destination: AdminDestination;
  iconClassName?: string;
}) {
  const match = useMatch({
    path: destination.path,
    end: destination.end ?? false,
  });
  const Icon = destination.icon;
  return (
    <SidebarMenuItem>
      <SidebarMenuButton
        asChild
        isActive={!!match}
        tooltip={destination.label}
        className={`${ACTIVE_STRIPE} text-xs text-sidebar-foreground/70 data-[active=true]:bg-sidebar-accent/40 data-[active=true]:font-normal`}
      >
        <NavLink to={destination.path} end={destination.end}>
          <Icon className={iconClassName} />
          <span>{destination.label}</span>
        </NavLink>
      </SidebarMenuButton>
    </SidebarMenuItem>
  );
}

function CollapsibleNavGroup({
  label,
  icon,
  items,
}: {
  label: string;
  icon: React.ReactNode;
  items: AdminDestination[];
}) {
  return (
    <Collapsible defaultOpen className="group/collapsible" asChild>
      <SidebarMenuItem>
        <CollapsibleTrigger asChild>
          <SidebarMenuButton
            tooltip={label}
            className="rounded-[3px] font-mono text-[10px] uppercase tracking-[0.08em] text-sidebar-foreground/65 data-[state=open]:bg-sidebar-accent/35 data-[state=open]:text-sidebar-accent-foreground"
          >
            {icon}
            <span>{label}</span>
            <ChevronRight className="ml-auto size-3.5 transition-transform duration-150 [transition-timing-function:cubic-bezier(0.23,1,0.32,1)] group-data-[state=open]/collapsible:rotate-90" />
          </SidebarMenuButton>
        </CollapsibleTrigger>
        <CollapsibleContent>
          <SidebarMenuSub className="mx-4 gap-0.5 border-sidebar-border/80 px-2 py-1">
            {items.map((item) => (
              <SubNavItem key={item.id} destination={item} />
            ))}
          </SidebarMenuSub>
        </CollapsibleContent>
      </SidebarMenuItem>
    </Collapsible>
  );
}

function SubNavItem({ destination }: { destination: AdminDestination }) {
  const match = useMatch({
    path: destination.path,
    end: destination.end ?? false,
  });
  const Icon = destination.icon;
  return (
    <SidebarMenuSubItem className="relative">
      <SidebarMenuSubButton
        asChild
        isActive={!!match}
        className={`${ACTIVE_STRIPE} h-8 text-xs text-sidebar-foreground/70`}
      >
        <NavLink to={destination.path} end={destination.end}>
          <Icon />
          <span>{destination.label}</span>
        </NavLink>
      </SidebarMenuSubButton>
    </SidebarMenuSubItem>
  );
}
