import { Command } from "cmdk";
import { Search } from "lucide-react";
import type { ReactNode } from "react";

import { BrandMark } from "@/components/lab/BrandMark";
import { cn } from "@/lib/utils";

type PaletteTone = "promoter" | "cds" | "rbs" | "terminator";

export function ProductCommandPalette({
  open,
  onOpenChange,
  value,
  onValueChange,
  eyebrow,
  description,
  placeholder,
  indexLabel,
  emptyTitle = "No matching commands",
  emptyDescription,
  children,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  value: string;
  onValueChange: (value: string) => void;
  eyebrow: string;
  description: string;
  placeholder: string;
  indexLabel: string;
  emptyTitle?: string;
  emptyDescription: string;
  children: ReactNode;
}) {
  if (!open) return null;

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={indexLabel}
      className="fixed inset-0 z-50 flex items-start justify-center bg-foreground/25 px-4 pt-[12vh] backdrop-blur-[2px]"
      onClick={() => onOpenChange(false)}
    >
      <Command
        label={indexLabel}
        className="relative w-full max-w-2xl overflow-hidden rounded-[4px] border border-foreground/20 bg-popover text-popover-foreground shadow-[0_24px_80px_hsl(var(--foreground)/0.24),0_2px_8px_hsl(var(--foreground)/0.12)]"
        onClick={(event) => event.stopPropagation()}
        loop
      >
        <div aria-hidden="true" className="flex h-1">
          <span className="flex-1 bg-sbol-promoter" />
          <span className="flex-1 bg-sbol-cds" />
          <span className="flex-1 bg-sbol-rbs" />
          <span className="flex-1 bg-sbol-terminator" />
        </div>

        <header className="flex items-center gap-3 border-b bg-muted/20 px-4 py-3">
          <BrandMark className="size-8 shrink-0" />
          <div className="min-w-0">
            <p className="ledger-label text-primary">{eyebrow}</p>
            <p className="mt-0.5 truncate text-xs text-muted-foreground">
              {description}
            </p>
          </div>
          <kbd className="ml-auto rounded-[3px] border bg-background px-2 py-1 font-mono text-[10px] text-muted-foreground shadow-[0_1px_0_hsl(var(--foreground)/0.06)]">
            ⌘ K
          </kbd>
        </header>

        <div className="registry-field flex items-center gap-3 border-b px-4">
          <Search className="size-4 shrink-0 text-sbol-promoter" />
          <Command.Input
            autoFocus
            placeholder={placeholder}
            value={value}
            onValueChange={onValueChange}
            className="h-14 min-w-0 flex-1 border-0 bg-transparent p-0 text-[15px] text-foreground outline-none placeholder:text-muted-foreground/75"
          />
          <span className="hidden shrink-0 font-mono text-[9px] uppercase tracking-[0.12em] text-muted-foreground/60 sm:inline">
            {indexLabel}
          </span>
        </div>

        <Command.List className="max-h-[56vh] overflow-y-auto py-2">
          <Command.Empty className="px-5 py-10 text-center">
            <span className="mx-auto flex size-9 items-center justify-center rounded-[3px] border-l-2 border-sbol-promoter bg-primary/10 text-primary">
              <Search className="size-4" />
            </span>
            <span className="mt-3 block text-sm font-medium text-foreground">
              {emptyTitle}
            </span>
            <span className="mt-1 block text-xs text-muted-foreground">
              {emptyDescription}
            </span>
          </Command.Empty>
          {children}
        </Command.List>

        <div className="flex flex-wrap items-center gap-x-4 gap-y-2 border-t bg-muted/20 px-4 py-2 text-[10px] text-muted-foreground">
          <span className="ledger-label mr-auto hidden text-muted-foreground/65 sm:inline">
            SBOL DB {indexLabel}
          </span>
          <PaletteHint keys="↑↓" label="Navigate" />
          <PaletteHint keys="↵" label="Select" />
          <PaletteHint keys="esc" label="Close" />
        </div>
      </Command>
    </div>
  );
}

export function ProductCommandPaletteGroup({
  heading,
  tone,
  children,
}: {
  heading: string;
  tone: PaletteTone;
  children: ReactNode;
}) {
  const toneClass: Record<PaletteTone, string> = {
    promoter: "bg-sbol-promoter",
    cds: "bg-sbol-cds",
    rbs: "bg-sbol-rbs",
    terminator: "bg-sbol-terminator",
  };

  return (
    <Command.Group
      heading={
        <span className="flex items-center gap-2">
          <span className={cn("size-1.5 rounded-full", toneClass[tone])} />
          <span className="ledger-label text-muted-foreground">{heading}</span>
          <span className="h-px flex-1 bg-border/70" />
        </span>
      }
      className="py-1 [&_[cmdk-group-heading]]:px-4 [&_[cmdk-group-heading]]:pb-1.5 [&_[cmdk-group-heading]]:pt-2"
    >
      {children}
    </Command.Group>
  );
}

export function ProductCommandPaletteItem({
  icon,
  label,
  trailing,
  mono = false,
  onSelect,
}: {
  icon: ReactNode;
  label: string;
  trailing?: string;
  mono?: boolean;
  onSelect: () => void;
}) {
  return (
    <Command.Item
      onSelect={onSelect}
      className="group mx-2 flex min-h-10 cursor-pointer items-center gap-3 rounded-[3px] border border-transparent border-l-2 px-2.5 py-1.5 text-sm text-foreground outline-none aria-selected:border-l-primary aria-selected:bg-accent/70 aria-selected:text-accent-foreground"
    >
      <span className="flex size-7 shrink-0 items-center justify-center rounded-[3px] border bg-card text-muted-foreground group-aria-selected:border-primary/25 group-aria-selected:bg-primary/10 group-aria-selected:text-primary">
        {icon}
      </span>
      <span className={cn("flex-1 truncate", mono && "font-mono text-xs")}>
        {label}
      </span>
      {trailing && (
        <span className="shrink-0 rounded-[3px] border bg-background/70 px-1.5 py-0.5 font-mono text-[9px] uppercase tracking-[0.06em] text-muted-foreground">
          {trailing}
        </span>
      )}
    </Command.Item>
  );
}

function PaletteHint({ keys, label }: { keys: string; label: string }) {
  return (
    <span className="inline-flex items-center gap-1.5">
      <kbd className="rounded-[2px] border bg-background px-1.5 py-0.5 font-mono text-[9px] text-foreground shadow-[0_1px_0_hsl(var(--foreground)/0.06)]">
        {keys}
      </kbd>
      <span>{label}</span>
    </span>
  );
}
