# Domain model

`sbol-db` stores synthetic-biology designs in two layers joined by one key (the
object IRI):

1. **A verbatim RDF triplestore** (the source of truth): named **graphs** of
   **triples**. SPARQL and the SynBioHub-compatible endpoints read this layer.
2. **A derived, typed SBOL view** (an index for the native query primitives):
   generic **objects** plus typed projections (**components**, **sequences**,
   **features**, **locations**, ...). `object get`, neighborhood traversal,
   ontology-aware role filtering, and sequence search read this layer.

The graph owns the triples; the typed view is derived from those triples; the
IRI threads through all of it.

```
STORAGE (RDF truth)                      DERIVED VIEW (typed index)
─────────────────────                    ──────────────────────────
sbol_graphs ──owns──▶ sbol_triples         sbol_objects ──typed facet──▶ sbol_components
  iri (PK)             graph_iri (FK)       id (PK)                      object_id (PK/FK)
  id  (uuid)           subject_iri          iri  ◀═══ same IRI ═══▶      iri
  kind: sbol3|verbatim predicate_iri        sbol_class                  types[], roles[]
  name, format, hash…  object_*             graph_id ─┐              sequence_iris[]
                                            …            │              feature_iris[] …
        ▲                                                │           (+ sbol_sequences,
        └───────────── derived from ────────────────────┘             sbol_features,
              (sbol_objects.graph_id ─FK▶ sbol_graphs.id)           sbol_locations …)
```

| Table | Key column | References | On delete | Meaning |
| --- | --- | --- | --- | --- |
| `sbol_triples` | `graph_iri` | `sbol_graphs(iri)` | CASCADE | a triple is **owned** by its graph |
| `sbol_objects` | `graph_id` | `sbol_graphs(id)` | SET NULL | which graph the object was **derived from** |
| `sbol_components` | `object_id` | `sbol_objects(id)` | CASCADE | a Component is the **typed facet** of an object |
| `sbol_sequences` / `sbol_features` / `sbol_locations` / ... | `object_id` | `sbol_objects(id)` | CASCADE | one typed facet per SBOL class |

## A worked biological example

The file [`docs/examples/reporter_unit.ttl`](examples/reporter_unit.ttl) is a
constitutive reporter unit whose first 35 bp is the Anderson constitutive
promoter **J23100** (a real iGEM part, BBa_J23100), modeled as a located
sub-part of the larger device:

```turtle
:reporter_unit  a sbol:Component ;          # the device (engineered region)
    sbol:role        SO:0000804 ;           # engineered region
    sbol:hasSequence :reporter_seq ;
    sbol:hasFeature  <reporter_unit/promoter> .

<reporter_unit/promoter>  a sbol:SubComponent ;
    sbol:instanceOf  :J23100 ;              # ...which IS the J23100 promoter
    sbol:hasLocation <reporter_unit/promoter/loc> .

<reporter_unit/promoter/loc>  a sbol:Range ;
    sbol:start "1" ; sbol:end "35" .        # located at bp 1-35

:J23100  a sbol:Component ;                 # the promoter part
    sbol:role        SO:0000167 ;           # promoter
    sbol:hasSequence :J23100_seq .

:J23100_seq  a sbol:Sequence ;
    sbol:elements "ttgacggctagctcagtcctaggtacagtgctagc" .   # 35 bp
```

Import it (the schema applies on first run):

```sh
sbol-db graph import docs/examples/reporter_unit.ttl
# { "object_count": 6, "triple_count": 37, "validation_status": "passed", ... }
```

Everything below is the **actual** state that import produced.

### 1. `sbol_graphs` — the container (1 row)

```
iri                                                  | kind  | serialization_format
graph:document:5c56105e-f279-46be-a9f0-941d237e1244  | sbol3 | turtle
```

One `sbol3`-kind graph. Its `id` is the document's surrogate key; its `iri` is
where the triples live. (A `doc import` is, internally, just "create an `sbol3`
graph and write triples into it.")

### 2. `sbol_triples` — verbatim RDF (37 triples in that one graph)

`reporter_unit`'s eight triples, exactly as written, owned by the graph
(`graph_iri = graph:document:5c56105e…`):

```
predicate          | object
rdf:type           | sbol:Component
sbol:displayId     | "reporter_unit"
sbol:name          | "constitutive reporter unit"
sbol:type          | SBO:0000251               (DNA)
sbol:role          | SO:0000804                (engineered region)
sbol:hasSequence   | :reporter_seq
sbol:hasFeature    | :reporter_unit/promoter
sbol:hasNamespace  | :demo
```

This layer is uninterpreted RDF. A SPARQL query such as
`SELECT ?c WHERE { ?c a sbol:Component }` scans exactly here.

### 3. `sbol_objects` — derived generic view (6 rows)

The derivation step parses the graph's triples into SBOL objects and writes one
row per typed object, each stamped with `graph_id = the graph's id`:

```
iri                          | sbol_class       | display_id
:J23100                      | sbol:Component    | J23100
:reporter_unit               | sbol:Component    | reporter_unit
:J23100_seq                  | sbol:Sequence     | J23100_seq
:reporter_seq                | sbol:Sequence     | reporter_seq
:reporter_unit/promoter      | sbol:SubComponent | promoter
:reporter_unit/promoter/loc  | sbol:Range        | loc
```

### 4. `sbol_components` and siblings — typed facets

Each Component object gets a `sbol_components` row keyed by `object_id`; the RDF
edges become **indexable columns**:

```
iri            | roles          | feature_iris | sequence_iris
:reporter_unit | {SO:0000804}   | 1            | 1
:J23100        | {SO:0000167}   | 0            | 1
```

The other SBOL classes land in their own typed tables, all keyed by `object_id`
back to `sbol_objects`:

```
sbol_features:   :reporter_unit/promoter      kind=SubComponent  instanceOf=:J23100
sbol_locations:  :reporter_unit/promoter/loc  kind=Range  start=1  end=35
sbol_sequences:  :J23100_seq    alphabet=DNA  35 bp
                 :reporter_seq  alphabet=DNA  68 bp
sbol_sequence_kmers: 89 rows    (canonical 8-mers powering nucleotide search)
```

So the RDF edge `reporter_unit sbol:hasFeature …/promoter` is now a typed,
GIN-indexed `feature_iris` entry on the component **and** a `sbol_features` row
that points back to its parent and records `instanceOf = J23100`. The promoter's
`sbol:role SO:0000167` is a `roles` array you can filter with the ontology.

## Where they come together

For the single object `:J23100`, the **same IRI** materializes in three places,
bound by foreign key and provenance:

```
                ┌──────────────── sbol_graphs ────────────────┐
                │  iri = graph:document:5c56105e…             │
                │  id  = 5c56105e…    kind = sbol3            │
                └──────┬────────────────────────┬─────────────┘
        owns (graph_iri)│                        │ derived-from (graph_id)
                        ▼                        ▼
              sbol_triples (J23100's          sbol_objects
              triples + 30 more)             iri = :J23100
              ── SPARQL reads here ──        sbol_class = Component
                                             id = «obj-uuid»
                                                   │ object_id (FK, CASCADE)
                                                   ▼
                                             sbol_components
                                             iri = :J23100
                                             roles = {SO:0000167}
                                             ── native queries read here ──
```

- **Join key across layers:** the IRI string `…/demo/J23100`
  (`sbol_triples.subject_iri` = `sbol_objects.iri` = `sbol_components.iri`).
- **Ownership:** `sbol_triples.graph_iri → sbol_graphs.iri` (CASCADE).
- **Provenance:** `sbol_objects.graph_id → sbol_graphs.id` (SET NULL).
- **Typed facet:** `sbol_components.object_id → sbol_objects.id` (CASCADE).

## Read paths use opposite layers

| Query | Reads | How |
| --- | --- | --- |
| `query sparql 'SELECT ?c WHERE { ?c a sbol:Component }'` | `sbol_triples` | triple-pattern scan; returns `:J23100`, `:reporter_unit` |
| `object get …/J23100` | `sbol_objects` (+ typed tables) | by IRI |
| role-filtered search (`roles @> {SO:0000167}`) | `sbol_components` | GIN index on `roles` |
| `query neighborhood …/reporter_unit` | `sbol_triples` closure / `feature_iris` | bounded traversal |
| `query sequence-search GCTAGC` | `sbol_sequence_kmers` | canonical k-mer index |

## Lifecycle

The graph is the single thing you delete to remove an import:

```sql
DELETE FROM sbol_graphs WHERE id = '5c56105e-…';
```

This cascades to all 37 `sbol_triples`. The derived objects' `graph_id` is set
null (full derived-view cleanup is the reprojection's job). `doc delete` and
`DELETE /graphs/{id}` do exactly this against the `sbol3` graph.

## The verbatim (SynBioHub) variant

The same model serves the SynBioHub triplestore role, with one difference. A
graph written through the Graph Store / SPARQL Update endpoints has
`kind = 'verbatim'`: its triples are stored exactly as posted (no SBOL2 to SBOL3
upgrade), so `sbol_triples` is populated but the typed view
(`sbol_objects` / `sbol_components` / ...) is produced **asynchronously** by the
reprojection path rather than inline. The storage layer is identical; only the
timing and policy of the derivation differ. See
[`docs/synbiohub.md`](synbiohub.md).
