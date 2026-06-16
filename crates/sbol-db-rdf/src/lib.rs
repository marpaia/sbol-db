//! Projection helpers that bridge `sbol::Document` to the persistence layer.

mod export;
mod hash;
mod project;
mod projections;
mod summary;
mod vocab;

pub use export::{neighborhood_to_rdf, neighborhood_to_triples, triples_to_rdf};
pub use hash::{content_hash, hash_bytes};
pub use project::{document_to_triples, rdf_graph_to_triples, GRAPH_IRI_PREFIX};
pub use projections::document_to_projections;
pub use summary::{document_to_summaries, ObjectTriples};

use sbol_db_core::IriString;
use sbol_rdf::Resource;

pub(crate) fn subject_to_subject(resource: &Resource) -> sbol_db_core::triple::SubjectTerm {
    use sbol_db_core::triple::SubjectTerm;
    match resource {
        Resource::Iri(iri) => SubjectTerm::Iri(IriString::unchecked(iri.as_str())),
        Resource::BlankNode(node) => SubjectTerm::BlankNode(node.as_str().to_owned()),
        // `Resource` is `#[non_exhaustive]` upstream; future variants land
        // here. Fall back to a blank-node-style render so persistence still
        // succeeds rather than panicking.
        _ => SubjectTerm::BlankNode(format!("{resource}")),
    }
}
