# Registry discovery contract

Registry discovery is the shared search and browse contract behind the public
SBOL DB application. The application layer owns corpus scope, filtering,
ranking, sorting, exact totals, and paging. HTTP adapters translate wire values;
React serializes user choices into the URL and renders the response. Neither
adapter reimplements discovery semantics.

## Native API

`GET /api/v2/search` accepts the following query parameters:

| Parameter | Meaning |
| --- | --- |
| `q` | Free text over indexed object metadata. Omit it to browse the complete visible corpus. |
| `type` | Exact full `rdf:type` IRI. |
| `role` | Exact full SBOL 2 or SBOL 3 role IRI. |
| `collection` | Exact collection IRI; results must be direct members. |
| `owner` | Exact owner graph IRI from `sbh:ownedBy`. |
| `provenance` | Case-insensitive substring of mutable provenance. |
| `created_after`, `created_before` | Inclusive `YYYY-MM-DD` creation range. |
| `modified_after`, `modified_before` | Inclusive `YYYY-MM-DD` modification range. |
| `sort` | `relevance`, `name`, `created`, `modified`, or `iri`. |
| `direction` | `asc` or `desc`; name/IRI default ascending and other sorts default descending. |
| `offset` | Zero-based result offset; default `0`. |
| `limit` | Page size from `1` through `1000`; default `50`. |

All active filters are combined. Resource filters must be absolute IRIs. Date
ranges are inclusive and an inverted range is invalid. Every order ends with an
ascending IRI tie-breaker, so equal values cannot reshuffle across pages. The
response has this shape:

```json
{
  "items": [
    {
      "uri": "https://example.org/designs/pTet/1",
      "display_id": "pTet",
      "version": "1",
      "name": "Tet promoter",
      "description": "A repressible promoter",
      "object_type": "http://sbols.org/v2#ComponentDefinition",
      "roles": ["http://identifiers.org/so/SO:0000167"],
      "owners": ["https://example.org/user/alice"],
      "created_at": "2026-07-30",
      "modified_at": "2026-07-31",
      "score": 4.27
    }
  ],
  "total": 1,
  "offset": 0,
  "limit": 24,
  "sort": "relevance",
  "direction": "desc"
}
```

`total` is the exact size of the caller's authorized, filtered result set—not a
candidate-window estimate. `GET /api/v2/search/facets` returns exact visible
type and role counts with ontology labels and CURIEs when the corresponding
ontology term is loaded. A facet response never reveals values outside the
caller's graph scope.

## Public URL state

The public route at `/search` uses the same parameter names as the SynBioHub v2 API,
with three presentation-only additions:

- `view=grid|list` selects result density;
- `compat=classic` records that a classic link was translated; and
- repeated `compat_warning` values preserve user-visible translation losses.

Text, biological filters, advanced filters, sort direction, page size, and
offset therefore survive reload, back/forward navigation, and copying the URL.
The UI obtains type and role choices from `/api/v2/search/facets` and keeps
exact-IRI controls for terms that are valid but not in the visible facet list.
`/sequence-search` uses `q`, `mode=global|exact`, and `limit` for its dedicated
nucleotide workflow. Similarity is object-relative and begins from the
"Similar designs" section of a public object page.

## Classic link translation

Browser navigation to `/search/<classic-grammar>` is canonicalized to
`/search?...`. This is a UI compatibility translation only; non-HTML requests
continue to reach the V1-compatible machine handler.

| Classic key | Native parameter | Classification |
| --- | --- | --- |
| trailing free text | `q` | Semantically equivalent after removing standalone lowercase `and`, `or`, and `not`, matching the compatibility parser. |
| `objectType` | `type` | Bare and `sbol2:` names expand to the SBOL 2 namespace; `sbol3:` expands to SBOL 3; absolute IRIs pass through. |
| `sbol2:role`, `sbol3:role`, `role` | `role` | Exact role IRI. |
| `collection` | `collection` | Exact collection membership. |
| `sbh:ownedBy`, `ownedBy` | `owner` | Exact owner graph IRI. |
| `sbh:mutableProvenance`, `mutableProvenance` | `provenance` | Native matching is a case-insensitive substring. |
| `createdAfter`, `createdBefore` | `created_after`, `created_before` | Inclusive date bounds. |
| `modifiedAfter`, `modifiedBefore` | `modified_after`, `modified_before` | Inclusive date bounds. |
| arbitrary predicate equality | none | Intentionally not applied; the canonical page displays the omitted predicate and warning. |
| `sequence`, `globalsequence`, `exactsequence` | `/sequence-search?q=…&mode=…` | Redirected to the dedicated public nucleotide workflow. Other classic facets are named as ignored because the compatibility sequence handler also ignores them. |

An unsupported classic facet is never silently discarded. The SBOL DB page
states that the link was only partly translated, names every omitted filter,
and says that it was not applied. This avoids showing a broad result set as if
it satisfied a predicate the SynBioHub v2 contract does not implement.

## Ownership and maintenance

- `sbol-db-search` owns the ranked text index and full-match retrieval.
- `sbol-db-app` owns authorized metadata aggregation, biological filters,
  ontology labels, deterministic order, totals, and paging.
- `sbol-db-server` owns V2 parsing/error envelopes, OpenAPI, session scope, and
  the quarantined V1 path grammar.
- `features/portal` owns the typed client and URL parser/translator.
- shared UI primitives and `components/portal` own discovery controls and
  result representations; the route only composes the journey.

The phase gate and required evidence are defined in the
[application acceptance contract](application-acceptance.md#phase-1-discovery-parity).
