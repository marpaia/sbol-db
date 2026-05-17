/**
 * Tiny SVG sparkline. Renders a single area + line chart sized to fill
 * its container. Used by the observability page for per-bucket request
 * rate, latency quantiles, etc.
 *
 * No deps, no axes, no tooltips — just shape. The semantic detail
 * (timestamps, values) belongs in the KPI tile next to it.
 */

interface SparklineProps {
  points: number[];
  /** Pixel height. Width is set by the parent via CSS. */
  height?: number;
  /** Stroke + fill colour. Defaults to currentColor. */
  color?: string;
  /** Optional fixed Y-axis ceiling. If omitted, scales to data max. */
  max?: number;
  /** Optional aria-label for screen readers. */
  ariaLabel?: string;
}

export function Sparkline({
  points,
  height = 40,
  color = "currentColor",
  max,
  ariaLabel,
}: SparklineProps) {
  if (points.length === 0) {
    return (
      <div
        className="flex items-center justify-center text-[10px] text-muted-foreground/60"
        style={{ height }}
        aria-label={ariaLabel}
      >
        no data
      </div>
    );
  }

  const width = 200; // viewBox width; the SVG scales with CSS
  const padY = 2;
  const yMax = Math.max(max ?? 0, ...points, 1);
  const stepX = points.length > 1 ? width / (points.length - 1) : 0;

  const toY = (v: number) => {
    const span = height - padY * 2;
    return height - padY - (v / yMax) * span;
  };

  const path = points
    .map((v, i) => `${i === 0 ? "M" : "L"}${(i * stepX).toFixed(2)},${toY(v).toFixed(2)}`)
    .join(" ");

  const areaPath =
    `M0,${(height - padY).toFixed(2)} ` +
    points
      .map((v, i) => `L${(i * stepX).toFixed(2)},${toY(v).toFixed(2)}`)
      .join(" ") +
    ` L${((points.length - 1) * stepX).toFixed(2)},${(height - padY).toFixed(2)} Z`;

  return (
    <svg
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="none"
      width="100%"
      height={height}
      role="img"
      aria-label={ariaLabel}
      className="block"
    >
      <path d={areaPath} fill={color} opacity={0.12} />
      <path
        d={path}
        fill="none"
        stroke={color}
        strokeWidth={1.25}
        strokeLinejoin="round"
        strokeLinecap="round"
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}
