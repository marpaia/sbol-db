use serde::{Deserialize, Serialize};

use crate::IriString;

/// Object-position term in a triple: IRI, blank node, or RDF literal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObjectTerm {
    Iri(IriString),
    BlankNode(String),
    Literal {
        value: String,
        datatype: IriString,
        language: Option<String>,
    },
}

/// Subject-position term: IRI or blank node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubjectTerm {
    Iri(IriString),
    BlankNode(String),
}

/// An RDF triple, ready to persist into `sbol_triples`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Triple {
    pub graph_iri: Option<IriString>,
    pub subject: SubjectTerm,
    pub predicate: IriString,
    pub object: ObjectTerm,
}
