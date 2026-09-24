//! Object-listing inputs.

use sbol_db_core::GraphId;

/// Keyset-paginated object listing. Empty fields mean no restriction;
/// `after_iri` carries the last IRI of the prior page (lexicographic
/// ascending); `limit` is clamped to 1..=5000 and applied last.
#[derive(Clone, Debug, Default)]
pub struct ListObjectsFilter {
    pub sbol_class: Option<String>,
    pub role: Option<String>,
    /// Include objects occurring as a triple subject in this named graph,
    /// regardless of which import owns their derived record. An unknown graph
    /// matches nothing. Returned records remain the corpus-wide object view.
    pub graph_id: Option<GraphId>,
    /// Literal substring of the IRI, ignoring ASCII letter case. Non-ASCII
    /// characters remain case-sensitive; `%` and `_` are not wildcards. The
    /// value is not trimmed, and an empty string imposes no restriction.
    /// Matching happens before pagination and does not change IRI identity.
    pub iri_contains: Option<String>,
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
