# Graph neighborhood

The neighborhood primitive answers shape-of-graph questions like
"what's contained in this design?" and "what references this part?"
by walking `sbol_quads` outward from a root IRI under explicit
bounds.

It's the middle ground between `GET /objects?iri=…` (one row, no
context) and SPARQL (any query, any cost). The shape is fixed —
a bounded recursive traversal — but the parameters cover the common
exploration patterns without forcing the caller to write SPARQL.

## API

```rust
pub struct NeighborhoodQuery {
    pub root_iri: IriString,
    pub depth: u32,
    pub direction: Direction,             // Forward | Backward | Both
    pub predicate_allowlist: Vec<IriString>,
    pub max_nodes: Option<u32>,
    pub include_literals: bool,
}

pub struct NeighborhoodResult {
    pub root_iri: IriString,
    pub nodes: Vec<NodeInfo>,
    pub edges: Vec<EdgeInfo>,
    pub max_depth_reached: u32,
    pub truncated: bool,
}
```

`NodeInfo` carries the IRI, depth from the root, whether the node is
a blank node, and (for IRI nodes that resolve to a stored object)
the `sbol_class`, `display_id`, and `name` joined from
`sbol_objects`. `EdgeInfo` carries the subject / predicate / object
plus the depth at which the edge was traversed; the object position
is one of `Iri`, `BlankNode`, or `Literal { value, datatype, language }`.

## Direction

`Direction::Forward` follows edges where `subject == frontier →
object`. This is the "what does this design contain?" view. For an
SBOL Component, the forward neighborhood at depth 2 surfaces the
component's features and the features' parts.

`Direction::Backward` follows edges where `subject → object ==
frontier`. This is the "what references this thing?" view. The
backward neighborhood of a `Sequence` IRI surfaces every
`SubComponent` whose `instanceOf` points at the component that uses
it.

`Direction::Both` is the union; useful for dependency-closure-style
queries (`what is this design connected to in any direction?`).

## Bounds

Three knobs cap the traversal:

- `depth`: maximum traversal depth from the root. Depth 0 returns
  only the root node with no edges.
- `max_nodes`: hard cap on visited node count. If reached, the
  result has `truncated = true`.
- `predicate_allowlist`: when non-empty, only edges whose predicate
  IRI is in the set are followed. Restricts traversal without
  filtering the result client-side.

Literal-position edges are excluded from the recursive frontier by
default (a literal can't have outgoing edges). Set
`include_literals = true` to surface literal-typed properties of
visited subjects in the result's `edges` vec; the traversal frontier
is unaffected.

## CLI

```sh
sbol-db query neighborhood <iri>
  [--depth 2]
  [--direction forward|backward|both]
  [--predicate <iri>]...         # repeatable
  [--max-nodes 2048]
  [--literals]
  [--rdf turtle|jsonld|ntriples|rdfxml]
```

```sh
# What does this Component contain?
sbol-db query neighborhood https://synbiohub.org/public/igem/i13504 --depth 2

# What designs use this Sequence?
sbol-db query neighborhood https://synbiohub.org/public/igem/B0015 \
  --direction backward --depth 2

# Same, but restrict to structural composition.
sbol-db query neighborhood https://synbiohub.org/public/igem/i13504 \
  --predicate http://sbols.org/v3#hasFeature \
  --depth 3

# Emit the reached subgraph as Turtle.
sbol-db query neighborhood https://synbiohub.org/public/igem/i13504 \
  --depth 2 --rdf turtle
```

When `--rdf` is supplied, the visited edges are reassembled as RDF
in the requested format and that goes to stdout in place of the JSON
result. The RDF view is self-contained and re-parsable by `sbol-rs`.

## HTTP

```http
GET /objects/neighborhood?iri=<urlencoded>
  &depth=2
  &direction=forward
  &predicates=p1,p2          # comma-separated; serde_urlencoded does
                             # not support repeated keys
  &max_nodes=2048
  &literals=false
```

Returns the `NeighborhoodResult` as JSON.

The companion RDF route:

```http
GET /objects/neighborhood.rdf?iri=...&format=turtle
```

Returns the visited subgraph as RDF (defaults to Turtle, defaults
`literals=true` since RDF dumps usually want a faithful subgraph).
Content-Type matches the chosen format.

## Implementation

A single recursive CTE in `crates/sbol-db-postgres/src/repo/neighborhood.rs`
does the resource-position walk:

```sql
WITH RECURSIVE
edges AS (
    -- forward arm
    SELECT subject as from_id, ..., object as to_id, ...
    FROM sbol_quads
    WHERE $4 IN ('forward', 'both')
      AND (object_iri IS NOT NULL OR object_blank IS NOT NULL)

    UNION ALL

    -- backward arm
    SELECT object as from_id, ..., subject as to_id, ...
    FROM sbol_quads
    WHERE $4 IN ('backward', 'both') ...
),
walk AS (
    SELECT $1 as node_id, 0 as depth, ARRAY[$1] as path, ...
    UNION ALL
    SELECT e.to_id, w.depth + 1, w.path || e.to_id, ...
    FROM walk w JOIN edges e ON e.from_id = w.node_id
    WHERE w.depth < $2
      AND ($3::text[] IS NULL OR e.predicate = ANY($3))
      AND NOT e.to_id = ANY(w.path)
)
SELECT * FROM walk LIMIT $5
```

Postgres only allows one self-reference in a recursive term, so both
directions are normalized into a single non-recursive `edges` CTE
and the `walk` recursion walks `edges` instead of `sbol_quads`
directly. Cycle prevention is via the `path` array (`NOT to_id =
ANY(w.path)`).

Literal edges are gathered in a second non-recursive pass over the
visited subject set when `include_literals = true`. Node metadata
(`sbol_class`, `display_id`, `name`) joins to `sbol_objects` in a
third pass and decorates the IRI nodes in place.

## What's intentionally not here

- **Path explanation.** `EdgeInfo` carries the depth at which an
  edge was traversed but not the full path from the root. The path
  is computed inside the CTE for cycle prevention but not surfaced.
  Returning it would multiply result size in dense graphs.
- **Property paths.** Transitive paths (`sbol:hasFeature+`) are not
  expressed; use SPARQL with a property path if you need them.
- **Cross-direction predicate caps.** A single
  `predicate_allowlist` applies in both directions; per-direction
  allowlists would be cleaner for asymmetric walks but no caller
  has needed them yet.
