//! SPARQL-update write inputs and results.

use sbol_db_core::{IriString, Triple};

/// One resolved update operation, ready to apply atomically.
pub enum TripleChange {
    /// Delete then insert a set of triples.
    Change {
        deletes: Vec<Triple>,
        inserts: Vec<Triple>,
    },
    /// Clear a graph: `Some(iri)` is a named graph, `None` the default graph.
    Clear(Option<IriString>),
}

/// Tally of what an update changed.
#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct UpdateOutcome {
    pub inserted: usize,
    pub deleted: usize,
}
