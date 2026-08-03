import { ArrowUpRight, ScanLine } from "lucide-react";
import { useId, useState, type KeyboardEvent, type ReactNode } from "react";
import { Link } from "react-router-dom";

import cdsGlyph from "@/assets/sbol-glyphs/cds.svg";
import noGlyphAssigned from "@/assets/sbol-glyphs/no-glyph-assigned.svg";
import operatorGlyph from "@/assets/sbol-glyphs/operator.svg";
import originOfReplicationGlyph from "@/assets/sbol-glyphs/origin-of-replication.svg";
import promoterGlyph from "@/assets/sbol-glyphs/promoter.svg";
import ribosomeEntrySiteGlyph from "@/assets/sbol-glyphs/ribosome-entry-site.svg";
import terminatorGlyph from "@/assets/sbol-glyphs/terminator.svg";
import { ObjectSection } from "@/components/portal/ObjectSection";
import { SurfaceState } from "@/components/portal/SurfaceState";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import type {
  ObjectVisualFeature,
  ObjectVisualGlyph,
  PortalObjectDetails,
} from "@/features/portal/api";
import {
  compactRole,
  featureSequenceWindow,
  hasVisualRange,
  isReverseOrientation,
  layoutVisualFeatures,
  orientationLabel,
  sequencePreview,
  visualExtent,
  visualFeatureLength,
  visualGlyphForRoles,
  visualGlyphLabels,
  visualSpan,
} from "@/features/portal/visualization";
import { shortIri } from "@/features/portal/format";
import { usePortalObjectDetails } from "@/features/portal/queries";
import { publicObjectPath } from "@/lib/routes";
import { cn } from "@/lib/utils";

const VIEWBOX_WIDTH = 1_000;
const TRACK_START = 52;
const TRACK_END = 948;
const TRACK_WIDTH = TRACK_END - TRACK_START;
const RULER_Y = 38;
const FIRST_LANE_Y = 102;
const LANE_HEIGHT = 82;
const GLYPH_SIZE = 54;
const SBOL_GLYPH_VIEWBOX_SIZE = 45;
const ISOLATED_GLYPH_SIZE = 90;
const ISOLATED_BACKBONE_Y = 80;
const MAX_INLINE_SEQUENCE_LENGTH = 20_000;

interface GlyphSpec {
  asset: string;
  sourceBaseline: number;
  fillClass: string;
  textClass: string;
  dotClass: string;
}

const GLYPH_SPECS: Record<ObjectVisualGlyph, GlyphSpec> = {
  promoter: {
    asset: promoterGlyph,
    sourceBaseline: 39.746408,
    fillClass: "fill-sbol-promoter",
    textClass: "text-sbol-promoter",
    dotClass: "bg-sbol-promoter",
  },
  coding_sequence: {
    asset: cdsGlyph,
    sourceBaseline: 33.900582,
    fillClass: "fill-sbol-cds",
    textClass: "text-sbol-cds",
    dotClass: "bg-sbol-cds",
  },
  ribosome_entry_site: {
    asset: ribosomeEntrySiteGlyph,
    sourceBaseline: 32.499997,
    fillClass: "fill-sbol-rbs",
    textClass: "text-sbol-rbs",
    dotClass: "bg-sbol-rbs",
  },
  terminator: {
    asset: terminatorGlyph,
    sourceBaseline: 34.989391,
    fillClass: "fill-sbol-terminator",
    textClass: "text-sbol-terminator",
    dotClass: "bg-sbol-terminator",
  },
  operator: {
    asset: operatorGlyph,
    sourceBaseline: 32.499997,
    fillClass: "fill-sbol-rbs",
    textClass: "text-sbol-rbs",
    dotClass: "bg-sbol-rbs",
  },
  origin_of_replication: {
    asset: originOfReplicationGlyph,
    sourceBaseline: 22.499998,
    fillClass: "fill-sbol-promoter",
    textClass: "text-sbol-promoter",
    dotClass: "bg-sbol-promoter",
  },
  unspecified: {
    asset: noGlyphAssigned,
    sourceBaseline: 22.5,
    fillClass: "fill-muted-foreground",
    textClass: "text-muted-foreground",
    dotClass: "bg-muted-foreground",
  },
};

/**
 * Render the application-owned, coordinate-aware SBOL Visual projection. The
 * server decides whether the document is complete enough to draw; this view
 * never invents missing coordinates, order, orientation, or biological roles.
 */
export function ObjectVisualFallback({
  object,
}: {
  object: PortalObjectDetails;
}) {
  const visualization = object.visualization;

  // A design map is useful for Components, not as a repeated explanation on
  // every Sequence, Collection, Attachment, or provenance object page.
  if (visualization.state === "unsupported") return null;

  if (visualization.state === "empty") {
    return <LeafComponentView object={object} />;
  }

  return <ComponentDesignView object={object} />;
}

function LeafComponentView({ object }: { object: PortalObjectDetails }) {
  const visualization = object.visualization;
  const glyph = visualGlyphForRoles(object.roles);
  const spec = GLYPH_SPECS[glyph];
  const label = object.name || object.display_id || shortIri(object.iri);
  const sequenceReference =
    object.sequences.items.length === 1 ? object.sequences.items[0] : null;
  const sequenceIri =
    sequenceReference &&
    visualization.sequence_length !== null &&
    visualization.sequence_length <= MAX_INLINE_SEQUENCE_LENGTH
      ? sequenceReference.uri
      : "";
  const linkedSequence = usePortalObjectDetails(sequenceIri);

  return (
    <VisualSection description="An assertion-faithful component summary using official SBOL Visual 3.0 glyphs.">
      <div className="overflow-hidden border border-foreground/15 bg-background">
        <div className="flex items-center justify-between gap-4 border-b border-foreground/15 bg-muted/[0.08] px-4 py-2.5 text-[10px] text-muted-foreground sm:px-5">
          <span className="font-mono uppercase tracking-[0.12em]">
            Isolated component
          </span>
          <span>SBOL Visual 3.0</span>
        </div>

        <div className="grid md:grid-cols-[minmax(15rem,0.72fr)_minmax(0,1.28fr)] md:divide-x md:divide-foreground/15">
          <div className="flex min-h-52 flex-col items-center justify-center border-b border-foreground/15 bg-muted/[0.08] px-6 py-8 md:border-b-0">
            <div className="relative h-32 w-full max-w-64">
              <div
                className="absolute left-0 right-0 h-px bg-foreground/25"
                style={{ top: ISOLATED_BACKBONE_Y }}
              />
              <div
                role="img"
                aria-label={`${visualGlyphLabels[glyph]} glyph for ${label}`}
                className={cn("absolute bg-current", spec.textClass)}
                style={{
                  top:
                    ISOLATED_BACKBONE_Y -
                    (spec.sourceBaseline * ISOLATED_GLYPH_SIZE) /
                      SBOL_GLYPH_VIEWBOX_SIZE,
                  left: "50%",
                  width: ISOLATED_GLYPH_SIZE,
                  height: ISOLATED_GLYPH_SIZE,
                  transform: "translateX(-50%)",
                  maskImage: `url(${spec.asset})`,
                  WebkitMaskImage: `url(${spec.asset})`,
                  maskPosition: "center",
                  WebkitMaskPosition: "center",
                  maskRepeat: "no-repeat",
                  WebkitMaskRepeat: "no-repeat",
                  maskSize: "contain",
                  WebkitMaskSize: "contain",
                }}
              />
            </div>
            <p className="mt-3 text-center text-sm font-medium">{label}</p>
            <p className={cn("mt-1 text-xs font-medium", spec.textClass)}>
              {visualGlyphLabels[glyph]}
            </p>
          </div>

          <div className="min-w-0 px-4 py-6 sm:px-5">
            <p className="ledger-label text-muted-foreground">
              Component content
            </p>
            <h3 className="mt-1 text-lg font-medium tracking-tight">{label}</h3>
            <p className="mt-2 max-w-2xl text-xs leading-5 text-muted-foreground">
              This glyph represents the component’s asserted biological role. No
              child features, positions, or orientation are inferred.
            </p>

            <dl className="mt-5 grid gap-px overflow-hidden border bg-foreground/15 text-xs sm:grid-cols-3">
              <FeatureFact term="View" detail="Component level" />
              <FeatureFact
                term="Sequence"
                detail={
                  visualization.sequence_length === null
                    ? sequenceReference
                      ? "Linked"
                      : "Not asserted"
                    : `${visualization.sequence_length.toLocaleString()} bp`
                }
              />
              <FeatureFact term="Internal features" detail="Not asserted" />
            </dl>

            <div className="mt-5">
              <p className="font-mono text-[9px] uppercase tracking-[0.14em] text-muted-foreground">
                Visual role evidence
              </p>
              {object.roles.length ? (
                <div className="mt-2 flex flex-wrap gap-1.5">
                  {object.roles.map((role) => (
                    <Badge
                      key={role}
                      variant="outline"
                      className="font-mono text-[10px] font-normal"
                      title={role}
                    >
                      {compactRole(role)}
                    </Badge>
                  ))}
                </div>
              ) : (
                <p className="mt-2 text-xs leading-5 text-muted-foreground">
                  No biological role is asserted; the standard no-glyph-assigned
                  mark is used.
                </p>
              )}
            </div>
          </div>
        </div>

        {sequenceReference && (
          <LeafSequenceEvidence
            reference={sequenceReference}
            sequenceLength={visualization.sequence_length}
            sequence={linkedSequence.data || null}
            loading={linkedSequence.isLoading}
          />
        )}
      </div>
    </VisualSection>
  );
}

function LeafSequenceEvidence({
  reference,
  sequenceLength,
  sequence,
  loading,
}: {
  reference: PortalObjectDetails["sequences"]["items"][number];
  sequenceLength: number | null;
  sequence: PortalObjectDetails | null;
  loading: boolean;
}) {
  const elements = sequence?.sequence_content.elements || null;
  const preview = elements ? sequencePreview(elements) : null;
  const sequenceLabel =
    reference.name || reference.display_id || shortIri(reference.uri);

  return (
    <div className="border-t border-foreground/15 px-4 py-5 sm:px-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <p className="font-mono text-[9px] uppercase tracking-[0.14em] text-muted-foreground">
            Linked reference sequence
          </p>
          <p className="mt-1 text-xs text-muted-foreground">
            5′→3′ sequence evidence from the asserted SBOL Sequence resource.
          </p>
        </div>
        <Link
          to={publicObjectPath(reference.uri)}
          className="inline-flex min-h-9 items-center gap-2 rounded-md border px-3 text-xs font-medium text-muted-foreground transition-[background-color,color,border-color,transform] duration-150 hover:border-primary/30 hover:bg-accent hover:text-primary active:scale-[0.98] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none"
        >
          {sequenceLabel}
          <ArrowUpRight className="size-3.5" />
        </Link>
      </div>

      {loading ? (
        <Skeleton className="mt-4 h-20 w-full rounded-none" />
      ) : preview ? (
        <div className="mt-4 overflow-hidden border bg-muted/[0.08]">
          <div className="flex items-center justify-between gap-3 border-b px-3 py-2 font-mono text-[9px] text-muted-foreground">
            <span>Sequence excerpt</span>
            <span className="tabular-nums">
              {(
                sequence?.sequence_content.length ||
                sequenceLength ||
                0
              ).toLocaleString()}{" "}
              bases
            </span>
          </div>
          <code
            className="block break-all px-3 py-3 font-mono text-[11px] uppercase leading-6 tracking-[0.14em]"
            aria-label={`Sequence excerpt for ${sequenceLabel}`}
          >
            <span>{preview.head}</span>
            {preview.omitted > 0 && (
              <span className="px-1 tracking-normal text-muted-foreground">
                … {preview.omitted.toLocaleString()} bases …
              </span>
            )}
            <span>{preview.tail}</span>
          </code>
        </div>
      ) : (
        <p className="mt-4 border border-dashed px-3 py-3 text-xs leading-5 text-muted-foreground">
          {sequenceLength !== null &&
          sequenceLength > MAX_INLINE_SEQUENCE_LENGTH
            ? `The ${sequenceLength.toLocaleString()}-base sequence is linked but too large to load automatically.`
            : "The sequence relationship is asserted, but its elements are not available in this view."}
        </p>
      )}
    </div>
  );
}

function ComponentDesignView({ object }: { object: PortalObjectDetails }) {
  const visualization = object.visualization;
  const placed = visualization.features.filter(hasVisualRange);
  const [selectedUri, setSelectedUri] = useState<string | null>(null);
  const selected =
    visualization.features.find((feature) => feature.uri === selectedUri) ||
    placed[0] ||
    visualization.features[0];
  const unplacedCount = visualization.features.length - placed.length;
  const sequenceIri =
    object.sequences.items.length === 1 &&
    visualization.sequence_length !== null &&
    visualization.sequence_length <= MAX_INLINE_SEQUENCE_LENGTH
      ? object.sequences.items[0].uri
      : "";
  const linkedSequence = usePortalObjectDetails(sequenceIri);
  const sequenceElements =
    linkedSequence.data?.sequence_content.elements || null;

  return (
    <VisualSection>
      <div className="overflow-hidden border border-foreground/15 bg-background">
        <div className="grid gap-px border-b border-foreground/15 bg-foreground/15 sm:grid-cols-3">
          <DesignFact
            label="Sequence extent"
            value={`${visualExtent(
              visualization.features,
              visualization.sequence_length
            ).toLocaleString()} bp`}
          />
          <DesignFact
            label="Features"
            value={visualization.features.length.toLocaleString()}
          />
          <DesignFact
            label="Placement"
            value={unplacedCount ? `${unplacedCount} unresolved` : "Complete"}
            caution={unplacedCount > 0}
          />
        </div>

        {visualization.state === "partial" && (
          <div className="border-b border-warning/25 bg-warning/5 px-4 py-3 text-xs leading-5 text-muted-foreground sm:px-5">
            <span className="font-medium text-foreground">
              Partial coordinate view.{" "}
            </span>
            {visualization.note ||
              "Only asserted, valid feature ranges are placed; unresolved structure remains visible below."}
          </div>
        )}

        {placed.length ? (
          <DesignCanvas
            features={visualization.features}
            sequenceLength={visualization.sequence_length}
            selectedUri={selected?.uri || null}
            onSelect={setSelectedUri}
          />
        ) : (
          <SurfaceState
            variant="info"
            title="Feature metadata is available without positions"
            description="No feature has a complete addressable range, so SBOL DB will not draw a misleading layout."
            className="rounded-none border-0 border-b py-8"
          />
        )}

        <div className="grid border-t border-foreground/15 lg:grid-cols-[minmax(0,0.92fr)_minmax(18rem,0.58fr)] lg:divide-x lg:divide-foreground/15">
          {selected && (
            <FeatureInspector
              feature={selected}
              sequenceElements={sequenceElements}
            />
          )}
          <FeatureIndex
            features={visualization.features}
            selectedUri={selected?.uri || null}
            onSelect={setSelectedUri}
          />
        </div>
      </div>
    </VisualSection>
  );
}

function VisualSection({
  children,
  description = "An interactive, coordinate-faithful map using official SBOL Visual 3.0 glyphs.",
}: {
  children: ReactNode;
  description?: string;
}) {
  return (
    <ObjectSection
      id="visualization"
      icon={ScanLine}
      title="Design view"
      description={description}
    >
      {children}
    </ObjectSection>
  );
}

function DesignFact({
  label,
  value,
  caution = false,
}: {
  label: string;
  value: string;
  caution?: boolean;
}) {
  return (
    <div className="bg-background px-4 py-3 sm:px-5">
      <div className="font-mono text-[9px] uppercase tracking-[0.14em] text-muted-foreground">
        {label}
      </div>
      <div
        className={cn(
          "mt-1 text-sm font-medium tabular-nums",
          caution && "text-warning"
        )}
      >
        {value}
      </div>
    </div>
  );
}

function DesignCanvas({
  features,
  sequenceLength,
  selectedUri,
  onSelect,
}: {
  features: ObjectVisualFeature[];
  sequenceLength: number | null;
  selectedUri: string | null;
  onSelect: (uri: string) => void;
}) {
  const maximum = visualExtent(features, sequenceLength);
  const laidOut = layoutVisualFeatures(features);
  const laneCount = Math.max(...laidOut.map((item) => item.lane), 0) + 1;
  const height = FIRST_LANE_Y + laneCount * LANE_HEIGHT + 22;
  const idPrefix = `design-${useId().replaceAll(":", "")}`;
  const titleId = `${idPrefix}-title`;
  const descriptionId = `${idPrefix}-description`;

  return (
    <div className="bg-muted/[0.08]">
      <div className="flex items-center justify-between border-b border-foreground/10 px-4 py-2.5 text-[10px] text-muted-foreground sm:px-5">
        <span className="font-mono uppercase tracking-[0.12em]">
          Linear sequence map
        </span>
        <span>SBOL Visual 3.0</span>
      </div>
      <div className="overflow-x-auto">
        <svg
          viewBox={`0 0 ${VIEWBOX_WIDTH} ${height}`}
          className="block h-auto w-full min-w-[46rem]"
          role="img"
          aria-labelledby={`${titleId} ${descriptionId}`}
        >
          <title id={titleId}>SBOL Visual component map</title>
          <desc id={descriptionId}>
            {laidOut.length} positioned feature
            {laidOut.length === 1 ? "" : "s"} across {maximum} base pairs.
            Select a feature to inspect its asserted role, range, and
            orientation.
          </desc>
          <defs>
            {laidOut.map(({ feature }, index) => {
              const spec = GLYPH_SPECS[feature.glyph];
              return (
                <mask
                  key={feature.uri}
                  id={`${idPrefix}-glyph-${index}`}
                  x="0"
                  y="0"
                  width={GLYPH_SIZE}
                  height={GLYPH_SIZE}
                  maskUnits="userSpaceOnUse"
                  style={{ maskType: "alpha" }}
                >
                  <image
                    href={spec.asset}
                    x="0"
                    y="0"
                    width={GLYPH_SIZE}
                    height={GLYPH_SIZE}
                  />
                </mask>
              );
            })}
          </defs>

          <line
            x1={TRACK_START}
            x2={TRACK_END}
            y1={RULER_Y}
            y2={RULER_Y}
            className="stroke-foreground/15"
          />
          {rulerTicks(maximum).map(({ fraction, label }) => {
            const x = TRACK_START + TRACK_WIDTH * fraction;
            return (
              <g key={fraction}>
                <line
                  x1={x}
                  x2={x}
                  y1={RULER_Y - 4}
                  y2={RULER_Y + 6}
                  className="stroke-muted-foreground/45"
                />
                <text
                  x={x}
                  y={RULER_Y - 11}
                  textAnchor={
                    fraction === 0 ? "start" : fraction === 1 ? "end" : "middle"
                  }
                  className="fill-muted-foreground font-mono text-[9px] tabular-nums"
                >
                  {label}
                </text>
              </g>
            );
          })}

          {Array.from({ length: laneCount }).map((_, lane) => {
            const y = FIRST_LANE_Y + lane * LANE_HEIGHT;
            return (
              <line
                key={lane}
                x1={TRACK_START}
                x2={TRACK_END}
                y1={y}
                y2={y}
                className="stroke-foreground/25"
                strokeWidth="2"
                strokeLinecap="round"
              />
            );
          })}

          {laidOut.map(({ feature, lane }, index) => {
            const span = visualSpan(
              feature.start,
              feature.end,
              maximum,
              TRACK_WIDTH
            );
            const spec = GLYPH_SPECS[feature.glyph];
            const backboneY = FIRST_LANE_Y + lane * LANE_HEIGHT;
            const scale = GLYPH_SIZE / 45;
            const glyphCenter = TRACK_START + span.exactX + span.exactWidth / 2;
            const glyphX = Math.max(
              TRACK_START,
              Math.min(TRACK_END - GLYPH_SIZE, glyphCenter - GLYPH_SIZE / 2)
            );
            const glyphY = backboneY - spec.sourceBaseline * scale;
            const reverse = isReverseOrientation(feature.orientation);
            const selected = feature.uri === selectedUri;
            const hitX = Math.min(TRACK_START + span.x, glyphX) - 6;
            const hitRight = Math.max(
              TRACK_START + span.x + span.width,
              glyphX + GLYPH_SIZE
            );
            const hitWidth = hitRight - hitX + 12;
            const label = truncateLabel(feature.label);
            const range = `${feature.start.toLocaleString()}–${feature.end.toLocaleString()} bp`;
            const nearStart = glyphCenter < TRACK_START + 82;
            const nearEnd = glyphCenter > TRACK_END - 82;
            const labelX = nearStart
              ? TRACK_START
              : nearEnd
                ? TRACK_END
                : glyphCenter;
            const labelAnchor = nearStart
              ? "start"
              : nearEnd
                ? "end"
                : "middle";

            return (
              <g
                key={feature.uri}
                role="button"
                tabIndex={0}
                aria-label={`${feature.label}, ${visualGlyphLabels[feature.glyph]}, ${range}, ${orientationLabel(feature.orientation)}`}
                aria-pressed={selected}
                onClick={() => onSelect(feature.uri)}
                onKeyDown={(event) =>
                  selectOnKeyboard(event, feature.uri, onSelect)
                }
                className="group cursor-pointer outline-none"
              >
                <title>{`${feature.label} · ${range} · ${orientationLabel(feature.orientation)}`}</title>
                <rect
                  x={hitX}
                  y={backboneY - 48}
                  width={hitWidth}
                  height="73"
                  rx="6"
                  className={cn(
                    "fill-transparent stroke-transparent transition-[fill,stroke] duration-150 group-hover:fill-accent/35 group-focus-visible:stroke-ring motion-reduce:transition-none",
                    selected && "fill-primary/5 stroke-primary/30"
                  )}
                  strokeWidth="2"
                />
                <rect
                  x={TRACK_START + span.exactX}
                  y={backboneY - 4}
                  width={Math.max(1.5, span.exactWidth)}
                  height="8"
                  rx="4"
                  className={cn(
                    spec.fillClass,
                    selected ? "opacity-45" : "opacity-25"
                  )}
                />
                <g
                  transform={
                    reverse
                      ? `translate(${glyphX + GLYPH_SIZE} ${glyphY}) scale(-1 1)`
                      : `translate(${glyphX} ${glyphY})`
                  }
                >
                  <rect
                    x="0"
                    y="0"
                    width={GLYPH_SIZE}
                    height={GLYPH_SIZE}
                    className={spec.fillClass}
                    mask={`url(#${idPrefix}-glyph-${index})`}
                  />
                </g>
                <text
                  x={labelX}
                  y={backboneY + 25}
                  textAnchor={labelAnchor}
                  className={cn(
                    "fill-muted-foreground text-[10px] font-medium transition-colors duration-150 group-hover:fill-foreground motion-reduce:transition-none",
                    selected && "fill-foreground"
                  )}
                >
                  {label}
                </text>
              </g>
            );
          })}
        </svg>
      </div>
    </div>
  );
}

function FeatureInspector({
  feature,
  sequenceElements,
}: {
  feature: ObjectVisualFeature;
  sequenceElements: string | null;
}) {
  const spec = GLYPH_SPECS[feature.glyph];
  const length = visualFeatureLength(feature);
  return (
    <div className="min-w-0 px-4 py-5 sm:px-5">
      <div className="flex items-start gap-3">
        <span
          aria-hidden="true"
          className={cn("mt-1 size-2.5 shrink-0 rounded-full", spec.dotClass)}
        />
        <div className="min-w-0 flex-1">
          <p className="ledger-label text-muted-foreground">Selected feature</p>
          <h3 className="mt-1 truncate text-base font-medium tracking-tight">
            {feature.label}
          </h3>
          <p className={cn("mt-1 text-xs font-medium", spec.textClass)}>
            {visualGlyphLabels[feature.glyph]}
          </p>
        </div>
        <Link
          to={publicObjectPath(feature.uri)}
          className="flex size-9 shrink-0 items-center justify-center rounded-md border text-muted-foreground transition-[background-color,color,border-color,transform] duration-150 hover:border-primary/30 hover:bg-accent hover:text-primary active:scale-[0.97] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none"
          aria-label={`Open ${feature.label} object page`}
          title="Open feature object"
        >
          <ArrowUpRight className="size-4" />
        </Link>
      </div>

      <dl className="mt-5 grid gap-px overflow-hidden border bg-foreground/15 text-xs sm:grid-cols-3">
        <FeatureFact
          term="Range"
          detail={
            hasVisualRange(feature)
              ? `${feature.start.toLocaleString()}–${feature.end.toLocaleString()}`
              : "Unplaced"
          }
        />
        <FeatureFact
          term="Length"
          detail={length === null ? "—" : `${length.toLocaleString()} bp`}
        />
        <FeatureFact
          term="Strand"
          detail={orientationLabel(feature.orientation)}
        />
      </dl>

      <div className="mt-4">
        <p className="font-mono text-[9px] uppercase tracking-[0.14em] text-muted-foreground">
          Visual role evidence
        </p>
        {feature.roles.length ? (
          <div className="mt-2 flex flex-wrap gap-1.5">
            {feature.roles.map((role) => (
              <Badge
                key={role}
                variant="outline"
                className="font-mono text-[10px] font-normal"
                title={role}
              >
                {compactRole(role)}
              </Badge>
            ))}
          </div>
        ) : (
          <p className="mt-2 text-xs leading-5 text-muted-foreground">
            No biological role is asserted on this feature or its referenced
            definition; the standard no-glyph-assigned mark is used.
          </p>
        )}
      </div>
      {sequenceElements && (
        <SequenceEvidence feature={feature} elements={sequenceElements} />
      )}
    </div>
  );
}

function SequenceEvidence({
  feature,
  elements,
}: {
  feature: ObjectVisualFeature;
  elements: string;
}) {
  const window = featureSequenceWindow(elements, feature);
  if (!window) return null;
  const reverse = isReverseOrientation(feature.orientation);
  return (
    <div className="mt-5 border-t border-foreground/15 pt-4">
      <div className="flex items-center justify-between gap-3">
        <p className="font-mono text-[9px] uppercase tracking-[0.14em] text-muted-foreground">
          Reference sequence
        </p>
        <span className="font-mono text-[9px] tabular-nums text-muted-foreground">
          {window.start.toLocaleString()}–{window.end.toLocaleString()} bp
        </span>
      </div>
      <code className="mt-2 block break-all border bg-muted/20 px-3 py-2.5 font-mono text-[11px] uppercase leading-6 tracking-[0.15em]">
        {window.parts.map((part, index) => (
          <span
            key={`${part.kind}-${index}`}
            className={cn(
              part.kind === "flank" && "text-muted-foreground/65",
              part.kind === "feature" && "font-semibold text-primary",
              part.kind === "ellipsis" &&
                "px-1 tracking-normal text-muted-foreground"
            )}
          >
            {part.text}
          </span>
        ))}
      </code>
      <p className="mt-2 text-[10px] leading-4 text-muted-foreground">
        {reverse
          ? "Reference strand shown; this feature is asserted on the reverse-complement strand."
          : "Reference strand shown with the selected feature highlighted."}
      </p>
    </div>
  );
}

function FeatureFact({ term, detail }: { term: string; detail: string }) {
  return (
    <div className="min-w-0 bg-background px-3 py-2.5">
      <dt className="font-mono text-[8px] uppercase tracking-[0.12em] text-muted-foreground">
        {term}
      </dt>
      <dd className="mt-1 truncate font-medium tabular-nums" title={detail}>
        {detail}
      </dd>
    </div>
  );
}

function FeatureIndex({
  features,
  selectedUri,
  onSelect,
}: {
  features: ObjectVisualFeature[];
  selectedUri: string | null;
  onSelect: (uri: string) => void;
}) {
  return (
    <div className="min-w-0 border-t border-foreground/15 px-4 py-5 sm:px-5 lg:border-t-0">
      <div className="flex items-center justify-between gap-3">
        <p className="ledger-label text-muted-foreground">Feature index</p>
        <span className="font-mono text-[9px] text-muted-foreground">
          {features.length} total
        </span>
      </div>
      <div className="mt-3 max-h-80 space-y-1 overflow-y-auto pr-1">
        {features.map((feature) => {
          const spec = GLYPH_SPECS[feature.glyph];
          const selected = feature.uri === selectedUri;
          return (
            <button
              key={feature.uri}
              type="button"
              onClick={() => onSelect(feature.uri)}
              aria-pressed={selected}
              className={cn(
                "flex min-h-11 w-full items-center gap-3 rounded-md px-2.5 py-2 text-left transition-[background-color,color,transform] duration-150 hover:bg-accent/50 active:scale-[0.99] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none",
                selected && "bg-accent text-accent-foreground"
              )}
            >
              <span
                aria-hidden="true"
                className={cn("size-2 shrink-0 rounded-full", spec.dotClass)}
              />
              <span className="min-w-0 flex-1">
                <span className="block truncate text-xs font-medium">
                  {feature.label}
                </span>
                <span className="mt-0.5 block truncate font-mono text-[9px] text-muted-foreground">
                  {hasVisualRange(feature)
                    ? `${feature.start.toLocaleString()}–${feature.end.toLocaleString()} bp`
                    : "Unresolved position"}
                </span>
              </span>
              {!hasVisualRange(feature) && (
                <Badge
                  variant="outline"
                  className="shrink-0 border-warning/30 font-mono text-[9px] text-warning"
                >
                  unplaced
                </Badge>
              )}
            </button>
          );
        })}
      </div>
    </div>
  );
}

function rulerTicks(maximum: number) {
  return [0, 0.25, 0.5, 0.75, 1].map((fraction) => ({
    fraction,
    label: Math.max(1, Math.round(maximum * fraction)).toLocaleString(),
  }));
}

function selectOnKeyboard(
  event: KeyboardEvent,
  uri: string,
  onSelect: (uri: string) => void
) {
  if (event.key !== "Enter" && event.key !== " ") return;
  event.preventDefault();
  onSelect(uri);
}

function truncateLabel(label: string) {
  return label.length > 22 ? `${label.slice(0, 19)}…` : label;
}
