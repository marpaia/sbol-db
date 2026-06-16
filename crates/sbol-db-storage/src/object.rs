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
