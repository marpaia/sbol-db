import { ScanLine } from "lucide-react";
import type { ReactNode } from "react";
import { Link } from "react-router-dom";

import { ObjectSection } from "@/components/portal/ObjectSection";
import { SurfaceState } from "@/components/portal/SurfaceState";
import { Badge } from "@/components/ui/badge";
import type {
  ObjectVisualFeature,
  ObjectVisualGlyph,
  PortalObjectDetails,
} from "@/features/portal/api";
import { publicObjectPath } from "@/lib/routes";
import { cn } from "@/lib/utils";

const VIEWBOX_WIDTH = 1_000;
const TRACK_START = 52;
const TRACK_END = 948;
const TRACK_WIDTH = TRACK_END - TRACK_START;
const FEATURE_HEIGHT = 28;
const LANE_GAP = 14;

/**
 * Render the application-owned, coordinate-aware SBOL Visual subset. The
 * server decides whether the document is complete enough to draw; this
 * component never invents missing coordinates, order, orientation, or roles.
 */
export function ObjectVisualFallback({
  object,
}: {
  object: PortalObjectDetails;
}) {
  const visualization = object.visualization;
  if (visualization.state === "unsupported") {
    return (
      <VisualSection>
        <SurfaceState
          variant="unsupported"
          title="SBOL Visual does not apply to this object type"
          description={
            visualization.note ||
            "The identity, provenance, and exact RDF properties remain available without inferring a Component design."
          }
        />
      </VisualSection>
    );
  }
  if (visualization.state === "empty") {
    return (
      <VisualSection>
        <SurfaceState
          title="No feature structure is asserted"
          description={
            visualization.note ||
            "This Component can still be understood from its metadata and sequence relationships."
          }
        />
      </VisualSection>
    );
  }

  const placed = visualization.features.filter(hasRange);
  return (
    <VisualSection>
      <div className="space-y-5">
        {visualization.state === "partial" && (
          <SurfaceState
            variant="info"
            title="Partial coordinate view"
            description={
              visualization.note ||
              "Only asserted, valid feature ranges are drawn. Unplaced features remain listed so incomplete biology is visible."
            }
            className="py-5 text-left [&>span]:mx-0 [&>p]:mx-0"
          />
        )}
        {placed.length > 0 ? (
          <DesignTrack
            features={placed}
            sequenceLength={visualization.sequence_length}
          />
        ) : (
          <SurfaceState
            variant="info"
            title="Feature metadata is available without positions"
            description="No feature has a complete addressable range, so the design remains metadata-first rather than showing a misleading layout."
          />
        )}
        <FeatureLegend features={visualization.features} />
      </div>
    </VisualSection>
  );
}

function VisualSection({ children }: { children: ReactNode }) {
  return (
    <ObjectSection
      id="visualization"
      icon={ScanLine}
      title="Design view"
      description="A coordinate-aware SBOL Visual overview derived only from asserted feature roles, ranges, and orientation."
    >
      {children}
    </ObjectSection>
  );
}

function DesignTrack({
  features,
  sequenceLength,
}: {
  features: ObjectVisualFeature[];
  sequenceLength: number | null;
}) {
  const maximum = Math.max(
    sequenceLength || 0,
    ...features.map((feature) => feature.end || 0),
    1
  );
  const laidOut = layoutFeatures(features);
  const lanes = Math.max(...laidOut.map((item) => item.lane), 0) + 1;
  const trackY = 48;
  const featuresY = 72;
  const height = featuresY + lanes * (FEATURE_HEIGHT + LANE_GAP) + 36;

  return (
    <div className="overflow-hidden rounded-xl border bg-background">
      <div className="flex items-center justify-between gap-4 border-b bg-muted/20 px-4 py-2.5 text-[11px] font-medium text-muted-foreground">
        <span>1 bp</span>
        <span>{maximum.toLocaleString()} bp</span>
      </div>
      <div className="overflow-x-auto">
        <svg
          viewBox={`0 0 ${VIEWBOX_WIDTH} ${height}`}
          className="block h-auto w-full min-w-[42rem]"
          role="img"
          aria-labelledby="design-view-title design-view-description"
        >
          <title id="design-view-title">SBOL Visual feature map</title>
          <desc id="design-view-description">
            {features.length} positioned feature
            {features.length === 1 ? "" : "s"} across {maximum} base pairs.
          </desc>
          <line
            x1={TRACK_START}
            x2={TRACK_END}
            y1={trackY}
            y2={trackY}
            className="stroke-border"
            strokeWidth="3"
            strokeLinecap="round"
          />
          {[0, 0.25, 0.5, 0.75, 1].map((fraction) => (
            <g key={fraction}>
              <line
                x1={TRACK_START + TRACK_WIDTH * fraction}
                x2={TRACK_START + TRACK_WIDTH * fraction}
                y1={trackY - 5}
                y2={trackY + 5}
                className="stroke-muted-foreground/50"
              />
              <text
                x={TRACK_START + TRACK_WIDTH * fraction}
                y={trackY - 12}
                textAnchor={
                  fraction === 0 ? "start" : fraction === 1 ? "end" : "middle"
                }
                className="fill-muted-foreground text-[10px]"
              >
                {Math.max(1, Math.round(maximum * fraction)).toLocaleString()}
              </text>
            </g>
          ))}
          {laidOut.map(({ feature, lane }) => {
            const start = feature.start || 1;
            const end = feature.end || start;
            const x = TRACK_START + ((start - 1) / maximum) * TRACK_WIDTH;
            const width = Math.max(
              18,
              ((end - start + 1) / maximum) * TRACK_WIDTH
            );
            const y = featuresY + lane * (FEATURE_HEIGHT + LANE_GAP);
            const reverse = isReverse(feature.orientation);
            return (
              <g key={feature.uri}>
                <title>{`${feature.label}, ${start}–${end} bp${reverse ? ", reverse complement" : ""}`}</title>
                <g
                  transform={
                    reverse
                      ? `translate(${x + width} 0) scale(-1 1)`
                      : `translate(${x} 0)`
                  }
                  className="text-primary"
                >
                  <Glyph kind={feature.glyph} width={width} y={y} />
                </g>
                <text
                  x={x + width / 2}
                  y={y + FEATURE_HEIGHT + 12}
                  textAnchor="middle"
                  className="fill-muted-foreground text-[10px]"
                >
                  {truncateLabel(feature.label)}
                </text>
              </g>
            );
          })}
        </svg>
      </div>
    </div>
  );
}

function Glyph({
  kind,
  width,
  y,
}: {
  kind: ObjectVisualGlyph;
  width: number;
  y: number;
}) {
  const middle = y + FEATURE_HEIGHT / 2;
  const bodyWidth = Math.max(8, width - 10);
  const shared = "fill-primary/15 stroke-primary";
  if (kind === "promoter") {
    return (
      <path
        d={`M 1 ${y + FEATURE_HEIGHT} V ${y + 4} H ${bodyWidth} M ${bodyWidth - 8} ${y} L ${bodyWidth} ${y + 4} L ${bodyWidth - 8} ${y + 8}`}
        className="fill-none stroke-primary"
        strokeWidth="2.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    );
  }
  if (kind === "coding_sequence") {
    return (
      <path
        d={`M 1 ${y + 3} H ${bodyWidth} L ${width - 1} ${middle} L ${bodyWidth} ${y + FEATURE_HEIGHT - 3} H 1 Z`}
        className={shared}
        strokeWidth="2"
      />
    );
  }
  if (kind === "ribosome_entry_site") {
    return (
      <path
        d={`M 1 ${middle} Q ${width / 2} ${y - 5} ${width - 1} ${middle}`}
        className="fill-none stroke-primary"
        strokeWidth="3"
        strokeLinecap="round"
      />
    );
  }
  if (kind === "terminator") {
    return (
      <path
        d={`M ${width / 2} ${y + 2} V ${y + FEATURE_HEIGHT} M 2 ${y + 2} H ${width - 2}`}
        className="fill-none stroke-primary"
        strokeWidth="3"
        strokeLinecap="round"
      />
    );
  }
  if (kind === "operator") {
    return (
      <rect
        x="1"
        y={y + 3}
        width={Math.max(12, width - 2)}
        height={FEATURE_HEIGHT - 6}
        rx="2"
        className={shared}
        strokeWidth="2"
      />
    );
  }
  if (kind === "origin_of_replication") {
    return (
      <circle
        cx={width / 2}
        cy={middle}
        r={Math.min(FEATURE_HEIGHT / 2 - 2, width / 2)}
        className={shared}
        strokeWidth="2"
      />
    );
  }
  return (
    <rect
      x="1"
      y={y + 3}
      width={Math.max(12, width - 2)}
      height={FEATURE_HEIGHT - 6}
      rx="7"
      className="fill-muted stroke-muted-foreground"
      strokeWidth="1.5"
      strokeDasharray="4 3"
    />
  );
}

function FeatureLegend({ features }: { features: ObjectVisualFeature[] }) {
  return (
    <ul className="grid gap-2 sm:grid-cols-2">
      {features.map((feature) => (
        <li key={feature.uri}>
          <Link
            to={publicObjectPath(feature.uri)}
            className="group flex min-h-11 items-center justify-between gap-3 rounded-lg border px-3 py-2.5 transition-colors duration-150 hover:border-primary/30 hover:bg-muted/30 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 motion-reduce:transition-none"
          >
            <span className="min-w-0">
              <span className="block truncate text-sm font-medium group-hover:text-primary">
                {feature.label}
              </span>
              <span className="block truncate font-mono text-[10px] text-muted-foreground">
                {feature.uri}
              </span>
            </span>
            <Badge
              variant="outline"
              className={cn(
                "shrink-0 font-mono text-[10px]",
                !hasRange(feature) && "border-warning/30 text-warning"
              )}
            >
              {hasRange(feature)
                ? `${feature.start}–${feature.end}`
                : "unplaced"}
            </Badge>
          </Link>
        </li>
      ))}
    </ul>
  );
}

function layoutFeatures(features: ObjectVisualFeature[]) {
  const laneEnds: number[] = [];
  return [...features]
    .sort(
      (left, right) =>
        (left.start || 0) - (right.start || 0) ||
        (left.end || 0) - (right.end || 0) ||
        left.uri.localeCompare(right.uri)
    )
    .map((feature) => {
      const start = feature.start || 0;
      let lane = laneEnds.findIndex((end) => end < start);
      if (lane < 0) lane = laneEnds.length;
      laneEnds[lane] = feature.end || start;
      return { feature, lane };
    });
}

function hasRange(
  feature: ObjectVisualFeature
): feature is ObjectVisualFeature & { start: number; end: number } {
  return (
    feature.start !== null &&
    feature.end !== null &&
    feature.start > 0 &&
    feature.start <= feature.end
  );
}

function isReverse(orientation: string | null) {
  return Boolean(
    orientation &&
    /reverse(?:Complement|_complement)?$/i.test(
      orientation.replace(/[#/]/g, "")
    )
  );
}

function truncateLabel(label: string) {
  return label.length > 24 ? `${label.slice(0, 21)}…` : label;
}
