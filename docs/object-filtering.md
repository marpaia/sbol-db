# Embedded object filtering

Use `sbol_db_storage::ObjectStore::list_objects` for an object browser. The same
`ListObjectsFilter` works with SQLite, PostgreSQL, and RocksDB and returns the
existing `SbolObjectRecord` type. Clients do not need a separate connection,
knowledge of the storage schema, or a second object model.

```rust
use sbol_db_core::{DomainError, GraphId, SbolObjectRecord};
use sbol_db_storage::{ListObjectsFilter, ObjectStore};

async fn objects_page(
    store: &dyn ObjectStore,
    graph_id: Option<GraphId>,
    iri_query: Option<&str>,
    after_iri: Option<String>,
) -> Result<Vec<SbolObjectRecord>, DomainError> {
    store
        .list_objects(&ListObjectsFilter {
            graph_id,
            iri_contains: iri_query
                .map(str::trim)
                .filter(|query| !query.is_empty())
                .map(str::to_owned),
            after_iri,
            limit: 100,
            ..ListObjectsFilter::default()
        })
        .await
}
```

Add `sbol_class` and `role` to compose exact class/role filters with IRI matching.
All filters apply before the page limit. Results have distinct IRIs in ascending
IRI order; use the last returned IRI as `after_iri` for the next page and reset
the cursor when any filter changes. Limits are clamped to 1–5,000. A full page
can be the final page; an additional request can return no objects.

`iri_contains` is a literal substring of the entire IRI, ignoring ASCII letter
case. It does not search names, descriptions, or display IDs. `%` and `_` are
literal characters, not SQL wildcards. Non-ASCII characters retain their case;
IRIs are not decoded or normalized. `None` and an empty string impose no IRI
restriction. Whitespace is significant: the example trims a UI query explicitly.
The predicate does not change IRI identity or exact-IRI lookup semantics.

`graph_id` means the object occurs as a triple subject in the selected named
graph. Reimporting that object in another document does not hide it from the
first graph. A reference to an object as the object of a triple does not establish
subject membership. Unknown or deleted graphs return no matches. Records remain
the existing corpus-wide object view, not a reconstruction of graph-local
metadata; the returned record's `graph_id` is not a list of all its memberships.

The HTTP counterpart is `GET /objects/list`, with `iri_contains`, `graph_id`,
`sbol_class`, `role`, `after`, and `limit` query parameters. URL-encode literal
percent signs and other reserved query characters. It returns the existing
`objects` and `next_cursor` response shape.

## Updating an embedded consumer

This change adds `iri_contains` to `ListObjectsFilter`. Complete Rust struct
literals must add `iri_contains: None` or use `..ListObjectsFilter::default()`.
The SQL backends' `graph_id` behavior changes from the derived record's latest
import owner to membership in canonical triples. There is no legacy ownership
filter or second graph parameter. No storage migration or search-index rebuild
is required.

For GG Circuit, map the Tauri `iri_query` argument to `iri_contains`, pass its
existing graph ID and cursor, and convert the returned `SbolObjectRecord` through
the existing DTO conversion. This replaces the custom SQLite reader while
preserving combined filtering, graph membership, and pagination. Update the
related `sbol-db-*` dependencies together and regenerate `Cargo.lock` after a
crate release; the manifests in this branch retain the current release version.
