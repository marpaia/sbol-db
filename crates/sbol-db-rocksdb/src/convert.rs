//! Conversions between the storage contract's pattern/term types and Oxigraph's
//! `oxrdf` model, and from `oxrdf::Quad` back to the domain [`Triple`].
//!
//! `oxrdf` is the same version Oxigraph and the workspace share, so the term
//! types interoperate directly. A triple's named graph is its owner: the default
//! graph maps to `graph_iri == None`; a named-node graph to `Some(iri)`. A
//! blank-node graph name is not representable in the domain model and is an error.

use oxrdf::{GraphNameRef, Literal, NamedNode, NamedOrBlankNode, Quad, Term as OxTerm};
use sbol_db_core::{DomainError, IriString, ObjectTerm, SubjectTerm, Triple};
use sbol_db_storage::{GraphFilter, PatternObject, PatternSubject};

/// Owned subject node for a pattern scan; held so the borrowed `*Ref` passed to
/// `quads_for_pattern` outlives the call.
pub fn subject_node(subject: &PatternSubject) -> NamedOrBlankNode {
    match subject {
        PatternSubject::Iri(iri) => NamedOrBlankNode::NamedNode(NamedNode::new_unchecked(iri)),
        PatternSubject::Blank(b) => NamedOrBlankNode::BlankNode(oxrdf::BlankNode::new_unchecked(b)),
    }
}

/// Owned object term for a pattern scan.
pub fn object_term(object: &PatternObject) -> OxTerm {
    match object {
        PatternObject::Iri(iri) => OxTerm::NamedNode(NamedNode::new_unchecked(iri)),
        PatternObject::Blank(b) => OxTerm::BlankNode(oxrdf::BlankNode::new_unchecked(b)),
        PatternObject::Literal {
            value,
            datatype,
            language,
        } => OxTerm::Literal(match language {
            Some(lang) => Literal::new_language_tagged_literal_unchecked(value, lang),
            None => Literal::new_typed_literal(value, NamedNode::new_unchecked(datatype)),
        }),
    }
}

/// Owned predicate node for a pattern scan.
pub fn predicate_node(predicate: &str) -> NamedNode {
    NamedNode::new_unchecked(predicate)
}

/// The owned graph term a [`GraphFilter`] selects, if it pins one graph. `None`
/// means "no owned term to borrow" — the caller passes the matching
/// `Option<GraphNameRef>` separately (see [`graph_name_ref`]).
pub fn graph_node(filter: &GraphFilter) -> Option<NamedNode> {
    match filter {
        GraphFilter::Iri(g) => Some(NamedNode::new_unchecked(g)),
        GraphFilter::AnyNamed | GraphFilter::DefaultOnly => None,
    }
}

/// The `quads_for_pattern` graph argument for a [`GraphFilter`], borrowing
/// `node` (from [`graph_node`]) for the `Iri` case. `AnyNamed` cannot be
/// expressed as a single pattern bound, so it is `None` here and the caller
/// filters named-only results.
pub fn graph_name_ref<'a>(
    filter: Option<&GraphFilter>,
    node: Option<&'a NamedNode>,
) -> Option<GraphNameRef<'a>> {
    match filter {
        Some(GraphFilter::DefaultOnly) => Some(GraphNameRef::DefaultGraph),
        Some(GraphFilter::Iri(_)) => node.map(|n| GraphNameRef::NamedNode(n.as_ref())),
        Some(GraphFilter::AnyNamed) | None => None,
    }
}

/// `oxrdf::Quad` → domain [`Triple`]. The default graph yields `graph_iri ==
/// None`; a named graph yields `Some(iri)`; a blank-node graph name is rejected.
pub fn quad_to_triple(quad: &Quad) -> Result<Triple, DomainError> {
    let graph_iri = match &quad.graph_name {
        oxrdf::GraphName::DefaultGraph => None,
        oxrdf::GraphName::NamedNode(n) => Some(IriString::unchecked(n.as_str())),
        oxrdf::GraphName::BlankNode(_) => {
            return Err(DomainError::Database(
                "blank node graph name is not representable".into(),
            ))
        }
    };
    let subject = match &quad.subject {
        NamedOrBlankNode::NamedNode(n) => SubjectTerm::Iri(IriString::unchecked(n.as_str())),
        NamedOrBlankNode::BlankNode(b) => SubjectTerm::BlankNode(b.as_str().to_owned()),
    };
    let object = match &quad.object {
        OxTerm::NamedNode(n) => ObjectTerm::Iri(IriString::unchecked(n.as_str())),
        OxTerm::BlankNode(b) => ObjectTerm::BlankNode(b.as_str().to_owned()),
        OxTerm::Literal(l) => ObjectTerm::Literal {
            value: l.value().to_owned(),
            datatype: IriString::unchecked(l.datatype().as_str()),
            language: l.language().map(|s| s.to_owned()),
        },
    };
    Ok(Triple {
        graph_iri,
        subject,
        predicate: IriString::unchecked(quad.predicate.as_str()),
        object,
    })
}

/// An owned `oxrdf::Quad` for a domain [`Triple`], for insert/remove. The graph
/// is the triple's owner (`None` → default graph).
pub fn triple_to_quad(triple: &Triple) -> Quad {
    let subject = match &triple.subject {
        SubjectTerm::Iri(iri) => {
            NamedOrBlankNode::NamedNode(NamedNode::new_unchecked(iri.as_str()))
        }
        SubjectTerm::BlankNode(b) => {
            NamedOrBlankNode::BlankNode(oxrdf::BlankNode::new_unchecked(b))
        }
    };
    let object = match &triple.object {
        ObjectTerm::Iri(iri) => OxTerm::NamedNode(NamedNode::new_unchecked(iri.as_str())),
        ObjectTerm::BlankNode(b) => OxTerm::BlankNode(oxrdf::BlankNode::new_unchecked(b)),
        ObjectTerm::Literal {
            value,
            datatype,
            language,
        } => OxTerm::Literal(match language {
            Some(lang) => Literal::new_language_tagged_literal_unchecked(value, lang),
            None => Literal::new_typed_literal(value, NamedNode::new_unchecked(datatype.as_str())),
        }),
    };
    let graph_name = match &triple.graph_iri {
        Some(iri) => oxrdf::GraphName::NamedNode(NamedNode::new_unchecked(iri.as_str())),
        None => oxrdf::GraphName::DefaultGraph,
    };
    Quad {
        subject,
        predicate: NamedNode::new_unchecked(triple.predicate.as_str()),
        object,
        graph_name,
    }
}
