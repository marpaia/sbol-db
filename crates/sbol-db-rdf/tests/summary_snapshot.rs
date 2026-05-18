//! Snapshot test for `document_to_summaries`. If projection or
//! `triples_to_json` formatting changes, the snapshot diff makes the change
//! explicit.

use sbol::{Document, RdfFormat};
use sbol_db_rdf::document_to_summaries;

const FIXTURE: &str = include_str!("../../sbol-db-postgres/tests/fixtures/simple_component.ttl");

#[test]
fn simple_component_summary_snapshot() {
    let doc = Document::read(FIXTURE, RdfFormat::Turtle).expect("parse");
    let mut summaries: Vec<_> = document_to_summaries(&doc)
        .into_iter()
        .map(|oq| oq.summary)
        .collect();
    summaries.sort_by(|a, b| a.iri.as_str().cmp(b.iri.as_str()));

    // Don't snapshot the binary content_hash (it's redundant with the
    // hash_determinism test); rebuild summary records without it.
    let view: Vec<_> = summaries
        .iter()
        .map(|s| {
            serde_json::json!({
                "iri": s.iri.as_str(),
                "sbol_class": s.sbol_class,
                "display_id": s.display_id,
                "name": s.name,
                "description": s.description,
                "types": s.types,
                "roles": s.roles,
                "data": s.data,
            })
        })
        .collect();
    insta::assert_json_snapshot!("simple_component_summaries", view);
}
