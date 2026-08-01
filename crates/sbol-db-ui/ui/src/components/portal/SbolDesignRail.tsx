import { useId } from "react";

import { cn } from "@/lib/utils";

export function SbolDesignRail({
  className,
  compact = false,
}: {
  className?: string;
  compact?: boolean;
}) {
  const titleId = useId();
  const descriptionId = useId();
  return (
    <div
      className={cn(
        "overflow-hidden border-y border-foreground/15 bg-card/55",
        className
      )}
    >
      <svg
        viewBox="0 0 720 112"
        className={cn("block w-full", compact ? "h-12" : "h-24 sm:h-28")}
        role="img"
        aria-labelledby={`${titleId} ${descriptionId}`}
        preserveAspectRatio="xMidYMid slice"
      >
        <title id={titleId}>SBOL design feature rail</title>
        <desc id={descriptionId}>
          A promoter, ribosome entry site, coding sequence, operator, and
          terminator arranged on a coordinate rail.
        </desc>
        <g className="stroke-border" strokeWidth="1">
          {Array.from({ length: 13 }, (_, index) => (
            <line
              key={index}
              x1={40 + index * 54}
              x2={40 + index * 54}
              y1="22"
              y2="92"
              strokeDasharray="2 7"
            />
          ))}
        </g>
        <line
          x1="34"
          x2="686"
          y1="68"
          y2="68"
          className="stroke-foreground/35"
          strokeWidth="2"
        />
        <path
          d="M72 67V33h72m0 0-13-10m13 10-13 10"
          className="fill-none stroke-sbol-promoter"
          strokeWidth="3"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
        <path
          d="M184 68q29-42 58 0"
          className="fill-none stroke-sbol-rbs"
          strokeWidth="4"
          strokeLinecap="round"
        />
        <path
          d="M280 46h152l26 22-26 22H280z"
          className="fill-sbol-cds/15 stroke-sbol-cds"
          strokeWidth="3"
          strokeLinejoin="round"
        />
        <rect
          x="498"
          y="53"
          width="54"
          height="30"
          rx="2"
          className="fill-sbol-rbs/10 stroke-sbol-rbs"
          strokeWidth="2.5"
        />
        <path
          d="M616 30v38m-27-38h54"
          className="fill-none stroke-sbol-terminator"
          strokeWidth="3"
          strokeLinecap="round"
        />
        {!compact && (
          <g className="fill-muted-foreground font-mono text-[9px] tracking-[0.18em]">
            <text x="34" y="104">
              1 BP
            </text>
            <text x="654" y="104">
              DESIGN
            </text>
          </g>
        )}
      </svg>
    </div>
  );
}
