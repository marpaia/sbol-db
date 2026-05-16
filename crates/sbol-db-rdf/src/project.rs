use sbol::Document;
use sbol_db_core::{IriString, ObjectTerm, Quad};
use sbol_rdf::{Resource, Term, Triple};

use crate::subject_to_subject;

pub const GRAPH_IRI_PREFIX: &str = "graph:document:";

/// Convert every triple in `doc` into a domain `Quad` tagged with the given
/// `graph_iri`.
pub fn document_to_quads(doc: &Document, graph_iri: &IriString) -> Vec<Quad> {
    doc.rdf_graph()
        .triples()
        .iter()
        .map(|triple| triple_to_quad(triple, Some(graph_iri.clone())))
        .collect()
}

fn triple_to_quad(triple: &Triple, graph_iri: Option<IriString>) -> Quad {
    Quad {
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
