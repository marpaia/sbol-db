//! Graph-listing inputs.

use sbol_db_core::SerializationFormat;

/// Filter for a document-graph listing. Empty fields mean no restriction;
/// the limit is required and applied last.
#[derive(Clone, Debug, Default)]
pub struct ListGraphsFilter {
    /// Case-insensitive substring match against the graph's `name`.
    pub name: Option<String>,
    /// Exact match on the serialization format.
    pub format: Option<SerializationFormat>,
    /// Hard cap on the rows returned.
    pub limit: u32,
}
