import { cn } from "@/lib/utils";

/**
 * Square brand mark: a stylized DNA double helix on a teal backplate.
 * Mirrors the favicon so the in-app mark and the browser-tab mark
 * read as the same identity.
 */
export function BrandMark({ className }: { className?: string }) {
  return (
    <div
      className={cn(
        "flex aspect-square size-8 items-center justify-center rounded-lg bg-primary text-primary-foreground",
        className
      )}
      aria-hidden="true"
    >
      <svg
        xmlns="http://www.w3.org/2000/svg"
        viewBox="0 0 32 32"
        fill="none"
        stroke="currentColor"
        strokeWidth={2.2}
        strokeLinecap="round"
        strokeLinejoin="round"
        className="size-5"
      >
        {/* Two interlocking strands running left to right */}
        <path d="M5 9c5 0 6 14 11 14s6-14 11-14" />
        <path d="M5 23c5 0 6-14 11-14s6 14 11 14" />
        {/* Base-pair rungs */}
        <path d="M8 11v10" />
        <path d="M24 11v10" />
        <path d="M16 13v6" />
      </svg>
    </div>
  );
}
