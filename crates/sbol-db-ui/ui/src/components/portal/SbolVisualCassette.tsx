import { useId } from "react";

import cdsGlyph from "@/assets/sbol-glyphs/cds.svg";
import promoterGlyph from "@/assets/sbol-glyphs/promoter.svg";
import ribosomeEntrySiteGlyph from "@/assets/sbol-glyphs/ribosome-entry-site.svg";
import terminatorGlyph from "@/assets/sbol-glyphs/terminator.svg";

const SOURCE_GLYPH_HEIGHT = 45;
const DISPLAY_GLYPH_HEIGHT = 58;
const BACKBONE_Y = 72;

/**
 * The baseline coordinates come directly from the official SBOL Visual 3.0
 * glyph geometry. This component only lays out those assets; it does not draw
 * or approximate glyph shapes.
 */
const features = [
  {
    name: "Promoter",
    ontologyTerm: "SO:0000167",
    asset: promoterGlyph,
    x: 48,
    width: 58,
    sourceBaseline: 39.746408,
    fillClass: "fill-sbol-promoter",
  },
  {
    name: "RBS",
    ontologyTerm: "SO:0000139",
    asset: ribosomeEntrySiteGlyph,
    x: 166,
    width: 58,
    sourceBaseline: 32.499997,
    fillClass: "fill-sbol-rbs",
  },
  {
    name: "CDS",
    ontologyTerm: "SO:0000316",
    asset: cdsGlyph,
    x: 300,
    width: 58,
    sourceBaseline: 33.900582,
    fillClass: "fill-sbol-cds",
  },
  {
    name: "Terminator",
    ontologyTerm: "SO:0000141",
    asset: terminatorGlyph,
    x: 454,
    width: 58,
    sourceBaseline: 34.989391,
    fillClass: "fill-sbol-terminator",
  },
];

export function SbolVisualCassette() {
  const displayScale = DISPLAY_GLYPH_HEIGHT / SOURCE_GLYPH_HEIGHT;
  const maskPrefix = `cassette-glyph-${useId().replaceAll(":", "")}`;

  return (
    <div className="border-b border-foreground/15">
      <div className="flex items-center justify-between border-b border-foreground/10 px-4 py-2.5 sm:px-5">
        <span className="ledger-label text-muted-foreground">
          Expression cassette
        </span>
        <span className="font-mono text-[9px] text-primary">
          SBOL Visual 3.0
        </span>
      </div>
      <svg
        viewBox="0 0 560 132"
        className="block h-auto w-full"
        role="img"
        aria-labelledby="cassette-title cassette-description"
      >
        <title id="cassette-title">SBOL Visual expression cassette</title>
        <desc id="cassette-description">
          A promoter, ribosome entry site, coding sequence, and terminator
          arranged in order on a five-prime to three-prime nucleic acid
          backbone.
        </desc>
        <defs>
          {features.map((feature, index) => {
            const y = BACKBONE_Y - feature.sourceBaseline * displayScale;

            return (
              <mask
                key={feature.ontologyTerm}
                id={`${maskPrefix}-${index}`}
                x={feature.x}
                y={y}
                width={feature.width}
                height={DISPLAY_GLYPH_HEIGHT}
                maskUnits="userSpaceOnUse"
                style={{ maskType: "alpha" }}
              >
                <image
                  href={feature.asset}
                  x={feature.x}
                  y={y}
                  width={feature.width}
                  height={DISPLAY_GLYPH_HEIGHT}
                  preserveAspectRatio="none"
                />
              </mask>
            );
          })}
        </defs>
        <line
          x1="28"
          x2="532"
          y1={BACKBONE_Y}
          y2={BACKBONE_Y}
          className="stroke-foreground/40"
          strokeWidth="2"
        />
        <text
          x="28"
          y="62"
          className="fill-muted-foreground font-mono text-[9px]"
          textAnchor="start"
        >
          5′
        </text>
        <text
          x="532"
          y="62"
          className="fill-muted-foreground font-mono text-[9px]"
          textAnchor="end"
        >
          3′
        </text>
        {features.map((feature, index) => {
          const y = BACKBONE_Y - feature.sourceBaseline * displayScale;
          const center = feature.x + feature.width / 2;

          return (
            <g key={feature.ontologyTerm}>
              <rect
                x={feature.x}
                y={y}
                width={feature.width}
                height={DISPLAY_GLYPH_HEIGHT}
                className={feature.fillClass}
                mask={`url(#${maskPrefix}-${index})`}
              />
              <text
                x={center}
                y="103"
                textAnchor="middle"
                className="fill-foreground text-[10px] font-medium"
              >
                {feature.name}
              </text>
              <text
                x={center}
                y="118"
                textAnchor="middle"
                className="fill-muted-foreground font-mono text-[8px]"
              >
                {feature.ontologyTerm}
              </text>
            </g>
          );
        })}
      </svg>
    </div>
  );
}
