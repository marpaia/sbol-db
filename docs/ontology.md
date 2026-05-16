# Ontology expansion

SBOL leans heavily on external ontologies: the Sequence Ontology (SO)
classifies component and feature roles, the Systems Biology Ontology
(SBO) classifies interaction types, ChEBI / GO / EDAM / NCIT play
supporting roles. A useful SBOL query database has to expand role
filters along ontology hierarchies -- a query for "promoter" should
match every SO subclass of promoter, not just the literal IRI.

`sbol-db` loads OBO files from canonical URLs (OBO Foundry / EBI),
precomputes the transitive `is_a` closure, and exposes both raw term
lookup and closure queries. Role expansion against typed projections
(`sbol_components.roles`, `sbol_features.roles`, ...) is a single join
against `sbol_ontology_closure`.

## CLI

```sh
# Load SO and SBO from their canonical URLs.
sbol-db ontology fetch so
sbol-db ontology fetch sbo

# Load any other OBO ontology from an explicit URL.
sbol-db ontology fetch chebi \
  --url http://purl.obolibrary.org/obo/chebi/chebi_lite.obo \
  --name "Chemical Entities of Biological Interest"

# Inspect what's loaded.
sbol-db ontology list

# Every descendant of "promoter" (transitively).
sbol-db ontology descendants SO:0000167
sbol-db ontology descendants http://purl.obolibrary.org/obo/SO_0000167
sbol-db ontology descendants http://identifiers.org/so/SO:0000167
```

The `descendants` subcommand accepts the canonical OBO Foundry IRI,
the identifiers.org form (common in SBOL documents), or a bare CURIE
(`SO:0000167`). All three resolve to the same canonical term through
`sbol_ontology_term_aliases`.

## HTTP

```http
GET  /ontology                              # list loaded ontologies
POST /ontology       { "prefix": "so" }     # fetch + load
POST /ontology       { "prefix": "X", "url": "...", "name": "..." }
GET  /ontology/term?iri=SO:0000167          # term metadata
GET  /ontology/descendants?iri=SO:0000167   # closure
```

Both `term` and `descendants` accept either form of the IRI; the
canonicaliser runs first.

## What gets loaded

For each term in the OBO source:

- canonical IRI (`http://purl.obolibrary.org/obo/{PREFIX}_{NUMBER}`);
- `prefix` (uppercased: `SO`, `SBO`);
- CURIE (`SO:0000167`);
- name, optional definition, `is_obsolete` flag, synonym list.

For each `is_a` relation in-ontology, an entry is materialised in
`sbol_ontology_closure` along with the self-pair `(X, X, 0)`. Cross-ontology
parents are ignored -- closure stays within a single ontology by
design, so the descendant set of an SO term never silently includes
RO or BFO subclasses.

Two aliases are generated per term:

- the **identifiers.org** form (`http://identifiers.org/so/SO:0000167`)
  since SBOL documents commonly carry roles in that shape;
- any `alt_id` entries in the OBO term (former CURIEs that point at
  the current term).

Both alias forms resolve to the canonical IRI via
`sbol_ontology_term_aliases`.

## Architecture

Three tables drive expansion:

- `sbol_ontology_terms (iri, prefix, curie, name, definition, ...)` --
  one row per canonical term, keyed by IRI.
- `sbol_ontology_term_aliases (alias_iri, canonical_iri)` -- the
  redirection layer that lets a query for any historical IRI form
  hit the same term.
- `sbol_ontology_closure (ancestor_iri, descendant_iri, depth)` -- the
  precomputed transitive closure including self-pairs.

The closure is computed in Rust at load time via BFS upward from each
term through its `is_a` parents. The result is bulk-inserted via
`UNNEST` in a single round trip. For SO (~3,600 terms) the closure has
roughly 30 k pairs; for SBO (~700 terms) closer to 4 k.

A reload is a transactional replacement: `DELETE FROM sbol_ontologies
WHERE prefix = $1` cascades through every dependent table, the new
terms and closure go in, and the entire swap commits atomically. The
old prefix never appears alongside the new one.

## Where the closure lives in queries

Today the closure is exposed read-only: `ontology().descendants(iri)`
returns the descendant IRI set, which a caller can plug into
`WHERE roles && (SELECT array_agg(...))` style queries against the
typed projections. Wiring `?role=...&expand=so` into
`/components` / `/features` listing endpoints is the natural next
step but lives outside this document -- the closure table is the
durable surface.

## What's intentionally not here

- **Multi-relation closure.** Only `is_a` is followed. `part_of` and
  `has_role` are richer but rarely needed for the role-expansion
  shape SBOL queries want; they can be added by extending the closure
  builder if a use case appears.
- **Cross-ontology bridges.** Closure stops at the ontology boundary.
  A term that `is_a` something in another ontology produces no
  closure pair. Cross-ontology reasoning is out of scope.
- **Reasoner-derived inferences.** No OWL reasoner runs. What you
  load is what you get; if the OBO file omits an `is_a`, the closure
  omits it.
- **Bundled ontology data.** OBO files are not vendored into the
  repo. Loading goes through the live URL; pin versions externally
  if reproducibility matters.
