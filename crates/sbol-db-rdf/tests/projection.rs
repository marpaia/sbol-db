use sbol::{Document, RdfFormat};
use sbol_db_core::{IriString, ObjectTerm};
use sbol_db_rdf::{content_hash, document_to_summaries, document_to_triples, GRAPH_IRI_PREFIX};

const TURTLE: &str = r#"
BASE <https://example.org/test/>
PREFIX sbol: <http://sbols.org/v3#>
PREFIX SBO:  <https://identifiers.org/SBO:>
PREFIX SO:   <https://identifiers.org/SO:>
PREFIX EDAM: <https://identifiers.org/edam:>

<comp1>  a sbol:Component ;
    sbol:displayId    "comp1" ;
    sbol:hasNamespace <https://example.org/test> ;
    sbol:name         "Comp One" ;
    sbol:type         SBO:0000251 ;
    sbol:role         SO:0000167 ;
    sbol:hasSequence  <seq1> .

<seq1> a sbol:Sequence ;
    sbol:displayId    "seq1" ;
    sbol:hasNamespace <https://example.org/test> ;
    sbol:elements     "ATGC" ;
    sbol:encoding     EDAM:format_1207 .
"#;

#[test]
fn document_to_triples_round_trips_predicate_count() {
    let doc = Document::read(TURTLE, RdfFormat::Turtle).expect("parse");
    let graph = IriString::unchecked(format!("{GRAPH_IRI_PREFIX}abc"));
    let domain_triples = document_to_triples(&doc, &graph);
    let rdf_triples = doc.rdf_graph().triples();
    assert_eq!(domain_triples.len(), rdf_triples.len());
    for t in &domain_triples {
        assert_eq!(
            t.graph_iri.as_ref().map(|g| g.as_str()),
            Some(graph.as_str())
        );
    }
}

#[test]
fn document_to_summaries_extracts_types_and_roles() {
    let doc = Document::read(TURTLE, RdfFormat::Turtle).expect("parse");
    let summaries = document_to_summaries(&doc);
    assert_eq!(summaries.len(), 2, "one Component + one Sequence");
    let component = summaries
        .iter()
        .find(|s| s.summary.sbol_class == "http://sbols.org/v3#Component")
        .expect("component");
    assert_eq!(
        component.summary.types,
        vec!["https://identifiers.org/SBO:0000251"]
    );
    assert_eq!(
        component.summary.roles,
        vec!["https://identifiers.org/SO:0000167"]
    );
    assert_eq!(component.summary.display_id.as_deref(), Some("comp1"));
    assert_eq!(component.summary.name.as_deref(), Some("Comp One"));
}

#[test]
fn content_hash_is_deterministic_across_triple_order() {
    let doc = Document::read(TURTLE, RdfFormat::Turtle).expect("parse");
    let triples = doc.rdf_graph().triples().to_vec();
    let mut reversed = triples.clone();
    reversed.reverse();
    assert_eq!(content_hash(&triples), content_hash(&reversed));
}

#[test]
fn literal_triple_carries_datatype() {
    let doc = Document::read(TURTLE, RdfFormat::Turtle).expect("parse");
    let graph = IriString::unchecked(format!("{GRAPH_IRI_PREFIX}abc"));
    let triples = document_to_triples(&doc, &graph);
    let display_id_triple = triples
        .iter()
        .find(|q| q.predicate.as_str() == "http://sbols.org/v3#displayId")
        .expect("displayId triple");
    match &display_id_triple.object {
        ObjectTerm::Literal { datatype, .. } => {
            assert_eq!(datatype.as_str(), "http://www.w3.org/2001/XMLSchema#string");
        }
        other => panic!("expected literal, got {other:?}"),
    }
}
