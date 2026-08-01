import { cn } from "@/lib/utils";

/** A compact SBOL design signature: promoter, CDS, and terminator on one rail. */
export function BrandMark({ className }: { className?: string }) {
  return (
    <div
      className={cn(
        "flex aspect-square size-8 items-center justify-center rounded-[4px] border border-foreground/20 bg-card text-foreground shadow-[0_1px_0_hsl(var(--foreground)/0.08)]",
        className
      )}
      aria-hidden="true"
    >
      <svg
        xmlns="http://www.w3.org/2000/svg"
        viewBox="0 0 32 32"
        fill="none"
        strokeWidth={1.8}
        strokeLinecap="round"
        strokeLinejoin="round"
        className="size-5"
      >
        <path d="M3 23H29" className="stroke-foreground/35" />
        <path d="M5 22V10h6m0 0-3-3m3 3-3 3" className="stroke-sbol-promoter" />
        <path
          d="M13 16h7l4 4-4 4h-7z"
          className="fill-sbol-cds/15 stroke-sbol-cds"
        />
        <path d="M27 12v11m-4-11h8" className="stroke-sbol-terminator" />
      </svg>
    </div>
  );
}
