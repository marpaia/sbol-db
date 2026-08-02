import { cn } from "@/lib/utils";

/**
 * The official SBOL Visual 3.0 promoter glyph, used as the compact product
 * mark at sizes where a complete cassette would lose legibility.
 */
export function BrandMark({ className }: { className?: string }) {
  return (
    <div
      className={cn(
        "flex aspect-square size-8 items-center justify-center text-foreground",
        className
      )}
      aria-hidden="true"
    >
      <svg
        xmlns="http://www.w3.org/2000/svg"
        viewBox="0 0 45 45"
        fill="none"
        strokeWidth={3}
        strokeLinecap="round"
        strokeLinejoin="round"
        className="size-[78%] stroke-sbol-promoter"
      >
        <path d="m 29.000111,5.2464081 8.5,7.4999999 -8.5,7.3333" />
        <path d="m 7.5001114,39.746408 0,-27 28.9999996,0" />
      </svg>
    </div>
  );
}
