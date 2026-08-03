# SBOL Visual glyph assets

These SVG files are vendored from the official SBOL Visual Ontology web
service for SBOL Visual 3.0. They use the service's glyph-only response
(`specification=false`) and retain the official geometry.

| File | Sequence Ontology role | Source |
| --- | --- | --- |
| `promoter.svg` | `SO:0000167` | <https://vows.sbolstandard.org/glyph/SO:0000167/svg?version=3.0&specification=false> |
| `ribosome-entry-site.svg` | `SO:0000139` | <https://vows.sbolstandard.org/glyph/SO:0000139/svg?version=3.0&specification=false> |
| `cds.svg` | `SO:0000316` | <https://vows.sbolstandard.org/glyph/SO:0000316/svg?version=3.0&specification=false> |
| `terminator.svg` | `SO:0000141` | <https://vows.sbolstandard.org/glyph/SO:0000141/svg?version=3.0&specification=false> |
| `operator.svg` | `SO:0000057` | <https://vows.sbolstandard.org/glyph/SO:0000057/svg?version=3.0&specification=false> |
| `origin-of-replication.svg` | `SO:0000296` | <https://vows.sbolstandard.org/glyph/SO:0000296/svg?version=3.0&specification=false> |
| `no-glyph-assigned.svg` | no mapped role | <https://vows.sbolstandard.org/glyph/NoGlyphAssignedGlyph/svg?version=3.0&specification=false> |

Core glyphs were downloaded 2026-08-01; the operator, origin, and no-glyph
assets were downloaded 2026-08-03. Do not redraw or approximate standard glyphs. Add or
update assets only from an official SBOL Visual release or service endpoint.

The top-navigation `BrandMark` and `public/favicon.svg` reuse the exact promoter
path coordinates as the compact product mark. Stroke weight is adjusted for
legibility at favicon size, but the glyph geometry is unchanged. Its teal color
is SBOL DB presentation styling; SBOL Visual does not prescribe promoter color.
