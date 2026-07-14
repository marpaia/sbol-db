//! Object-listing inputs.

use sbol_db_core::GraphId;

/// Keyset-paginated object listing. Empty fields mean no restriction;
/// `after_iri` carries the last IRI of the prior page (lexicographic
/// ascending); `limit` is applied last.
#[derive(Clone, Debug, Default)]
pub struct ListObjectsFilter {
    pub sbol_class: Option<String>,
    pub role: Option<String>,
    pub graph_id: Option<GraphId>,
    pub after_iri: Option<String>,
    pub limit: u32,
}

/// Offset-paginated substring search over the derived object view. `text`
/// matches an object's `name`, `display_id`, or `description`; `sbol_class`
/// restricts by type; `property_uri` scopes the match to the literal value of
/// one predicate on the object rather than its summary fields. A `limit` of 0
/// asks for the total match count only, with no rows (the count-only path that
/// backs a "search count" query).
#[derive(Clone, Debug, Default)]
pub struct TextSearchQuery {
    pub text: String,
    pub sbol_class: Option<String>,
    pub property_uri: Option<String>,
    pub offset: i64,
    pub limit: i64,
}
