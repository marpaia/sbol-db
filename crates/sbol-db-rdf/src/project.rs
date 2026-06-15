use sbol::Document;
use sbol_db_core::{IriString, ObjectTerm, Triple};
use sbol_rdf::{Resource, Term, Triple as RdfTriple};

use crate::subject_to_subject;

pub const GRAPH_IRI_PREFIX: &str = "graph:document:";

/// Convert every triple in `doc` into a domain `Triple` tagged with the given
/// `graph_iri`.
pub fn document_to_triples(doc: &Document, graph_iri: &IriString) -> Vec<Triple> {
    doc.rdf_graph()
        .triples()
        .iter()
        .map(|triple| rdf_triple_to_triple(triple, Some(graph_iri.clone())))
        .collect()
}

/// Convert the triples of an already-parsed RDF graph into domain `Triple`s
/// tagged with `graph_iri`, verbatim.
///
/// Unlike [`document_to_triples`], this does no SBOL interpretation: it is the
/// ingest path for arbitrary RDF (e.g. SBOL2 posted by SynBioHub) where the
/// triples must be stored exactly as received, with no SBOL2→3 upgrade or
/// projection. The caller has already parsed bytes into a [`sbol_rdf::Graph`].
pub fn rdf_graph_to_triples(graph: &sbol_rdf::Graph, graph_iri: &IriString) -> Vec<Triple> {
    graph
        .triples()
        .iter()
        .map(|triple| rdf_triple_to_triple(triple, Some(graph_iri.clone())))
        .collect()
}

fn rdf_triple_to_triple(triple: &RdfTriple, graph_iri: Option<IriString>) -> Triple {
    Triple {
        graph_iri,
        subject: subject_to_subject(&triple.subject),
        predicate: IriString::unchecked(triple.predicate.as_str()),
        object: term_to_object(&triple.object),
    }
}

pub(crate) fn term_to_object(term: &Term) -> ObjectTerm {
    match term {
        Term::Resource(Resource::Iri(iri)) => ObjectTerm::Iri(IriString::unchecked(iri.as_str())),
        Term::Resource(Resource::BlankNode(node)) => {
            ObjectTerm::BlankNode(node.as_str().to_owned())
        }
        Term::Literal(literal) => ObjectTerm::Literal {
            value: literal.value().to_owned(),
            datatype: IriString::unchecked(literal.datatype().as_str()),
            language: literal.language().map(|s| s.to_owned()),
        },
        // Future-added `Resource` / `Term` variants fall back to a blank-node
        // render via `Resource`'s Display impl.
        Term::Resource(other) => ObjectTerm::BlankNode(format!("{other}")),
        _ => ObjectTerm::Literal {
            value: String::new(),
            datatype: IriString::unchecked("http://www.w3.org/2001/XMLSchema#string"),
            language: None,
        },
    }
}
