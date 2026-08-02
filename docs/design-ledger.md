# SBOL DB Design Ledger

The Design Ledger is SBOL DB's visual identity for the public registry, the
administrator control plane, and the API reference. It borrows its hierarchy
from scientific records and its color vocabulary from an SBOL design rail: a
promoter, ribosome binding site, coding sequence, and terminator connected on
one baseline.

The identity is product-owned. Deployment names remain secondary context and
legacy SynBioHub theme settings do not rename or recolor these surfaces.

## Design principles

1. **The record is primary.** Identity, structure, lineage, and representation
   are easier to scan than collections of interchangeable feature cards.
2. **Biology carries meaning.** SBOL glyphs and category colors identify real
   biological concepts. They are not decorative texture and never imply data
   that an API response did not provide.
3. **Public registry, private instrument.** The registry uses editorial display
   type and generous record-ledger spacing. Administration uses compact mono
   labels, tabular numbers, hard dividers, and status-focused density.
4. **One product, three surfaces.** Registry, admin, and API documentation share
   the signature mark, semantic palette, square geometry, focus treatment, and
   terminology while keeping layouts appropriate to their jobs.
5. **State must be explicit.** Empty, loading, partial, unavailable, error, and
   destructive states remain visibly distinct and accessible.

## Signature vocabulary

The SBOL DB mark is a compact design rail containing a promoter, coding
sequence, and terminator. The extended rail adds RBS, operator, coordinate
ticks, and labels when the available space can support them. Reusable rail and
fallback components must include an accessible text description.

| Concept | Token | Light reference | Intended use |
| --- | --- | --- | --- |
| Paper | `background`, `card` | warm white | Registry field and record surfaces |
| Ink | `foreground`, `sidebar` | blue-black | Text, rules, admin shell |
| Coding sequence | `cds`, `primary` | teal | Primary action and CDS meaning |
| Promoter | `promoter`, `warning` | orange | Promoter meaning and caution |
| RBS/operator | `rbs` | violet | RBS and operator meaning |
| Terminator | `terminator`, `destructive` | coral | Terminator meaning and destructive state |

Semantic feature colors must not be used to manufacture biological claims.
Status colors continue to communicate operational state, with adjacent text or
icons so color is never the only signal.

## Typography and geometry

- Public record titles and editorial headings use the `display` serif stack.
- Interface copy uses the sans stack; identities, coordinates, controls, and
  admin metadata use the mono stack where it improves parsing.
- Global radii stay between 2 and 6 pixels. Pills are reserved for true status
  or compact categorical values, not ordinary containers.
- Dividers and background bands establish groups before elevation does. Shadows
  are shallow and limited to interactive lift or overlays.
- Public content may breathe; admin query and operations views favor efficient
  density and stable alignment.

## Interaction contract

- Focus is visible and high contrast on every control.
- Hover explains clickability without moving surrounding layout.
- Pressed controls use a brief tactile scale; result arrows may travel a few
  pixels to show direction.
- Page-load animation, looping decoration, and broad `transition-all` behavior
  are not part of the identity.
- Reduced-motion preferences remove nonessential transitions.

## Surface application

### Registry

The home page introduces the corpus as a biological record, not a dashboard.
Search defaults to a scan-efficient ledger list. Object pages preserve one
record frame across summary, provenance, biological relationships, downloads,
and machine representations. Where real feature annotations are present, they
use semantic glyph color; where they are not, text states that limitation.

### Administrator control plane

The dark ink sidebar, mono section labels, semantic status rails, compact
workbench, and tabular metrics frame administration as an instrument. Query
editors and data tables retain density; settings and deliberate actions retain
the same shared form and feedback primitives.

### API reference

Both `/docs` and `/api/v2/docs` wrap Scalar in the shared product masthead and
override its stable design tokens with the paper, ink, rule, and CDS palette.
The interactive reference and its source switching remain Scalar-owned; the
product shell supplies navigation and identity.

## Acceptance checklist

- Registry, admin, and both API reference routes show the same product mark and
  semantic palette.
- Public navigation and search work at phone, tablet, and desktop widths.
- Search defaults to list view but preserves an explicit grid choice in the
  URL.
- Interactive controls have keyboard focus, accessible labels, and reduced
  motion behavior.
- Dark mode preserves contrast and biological color distinctions.
- No visualization invents an annotation, coordinate, provenance relationship,
  availability state, or download capability.
- UI unit tests, TypeScript checks, lint, production build, Rust docs contract
  tests, formatting, and rendered browser inspection pass before release.
