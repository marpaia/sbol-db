//! Postgres-backed [`spareval::QueryableDataset`] over `sbol_quads`.
//!
//! Each `internal_quads_for_pattern` call translates the bound positions to a
//! single SQL pattern scan via [`QuadRepository::scan_pattern`], buffers the
//! result rows, and returns an owning iterator. This is "buffer-per-pattern"
//! — fine for correctness, intentionally simple for v1.
//!
//! Because the `QueryableDataset` trait returns sync iterators but sqlx is
//! async, the implementation does
//! `tokio::runtime::Handle::current().block_on(...)` for each scan. This is
//! only safe when called from inside `tokio::task::spawn_blocking`, which is
//! exactly what [`crate::SparqlEngine`] arranges.

use std::sync::Arc;

use oxrdf::{BlankNode, Literal, NamedNode, Term};
use sbol_db_core::{DomainError, ObjectTerm, Quad, SubjectTerm};
use sbol_db_postgres::{GraphFilter, PatternObject, PatternSubject, QuadRepository};
use spareval::{InternalQuad, QueryableDataset};
use tokio::runtime::Handle;

/// Per-pattern row cap. Realistic SBOL queries have far fewer hits per pattern;
/// this is a safety valve against pathological scans.
const PATTERN_LIMIT: i64 = 1_000_000;

#[derive(Clone)]
pub struct PostgresDataset {
    quads: Arc<QuadRepository>,
}

impl PostgresDataset {
    pub fn new(quads: Arc<QuadRepository>) -> Self {
        Self { quads }
    }
}

impl<'a> QueryableDataset<'a> for &'a PostgresDataset {
    type InternalTerm = Term;
    type Error = DomainError;

    fn internal_quads_for_pattern(
        &self,
        subject: Option<&Term>,
        predicate: Option<&Term>,
        object: Option<&Term>,
        graph_name: Option<Option<&Term>>,
    ) -> impl Iterator<Item = Result<InternalQuad<Term>, DomainError>> + use<'a> {
        // Translate the bound positions. Any unsupported position (literal in
        // subject/graph, non-IRI predicate, etc.) shortcuts to an empty iter.
        let pattern_subject = match subject {
            None => None,
            Some(Term::NamedNode(n)) => Some(PatternSubject::Iri(n.as_str().to_owned())),
            Some(Term::BlankNode(b)) => Some(PatternSubject::Blank(b.as_str().to_owned())),
            Some(_) => return Vec::new().into_iter(),
        };
        let pattern_predicate = match predicate {
            None => None,
            Some(Term::NamedNode(n)) => Some(n.as_str().to_owned()),
            Some(_) => return Vec::new().into_iter(),
        };
        let pattern_object = match object {
            None => None,
            Some(Term::NamedNode(n)) => Some(PatternObject::Iri(n.as_str().to_owned())),
            Some(Term::BlankNode(b)) => Some(PatternObject::Blank(b.as_str().to_owned())),
            Some(Term::Literal(lit)) => Some(PatternObject::Literal {
                value: lit.value().to_owned(),
                datatype: lit.datatype().as_str().to_owned(),
                language: lit.language().map(str::to_owned),
            }),
        };
        let pattern_graph = match graph_name {
            None => Some(GraphFilter::AnyNamed),
            Some(None) => Some(GraphFilter::DefaultOnly),
            Some(Some(Term::NamedNode(g))) => Some(GraphFilter::Iri(g.as_str().to_owned())),
            Some(Some(_)) => return Vec::new().into_iter(),
        };

        // Run the async scan to completion synchronously. Only valid inside
        // `spawn_blocking`; see module-level docs.
        let repo = Arc::clone(&self.quads);
        let result = Handle::current().block_on(async move {
            repo.scan_pattern(
                pattern_subject.as_ref(),
                pattern_predicate.as_deref(),
                pattern_object.as_ref(),
                pattern_graph.as_ref(),
                PATTERN_LIMIT,
            )
            .await
        });

        match result {
            Ok(quads) => quads
                .into_iter()
                .map(quad_to_internal_quad)
                .collect::<Vec<_>>()
                .into_iter(),
            Err(e) => vec![Err(e)].into_iter(),
        }
    }

    fn internalize_term(&self, term: Term) -> Result<Term, DomainError> {
        Ok(term)
    }

    fn externalize_term(&self, term: Term) -> Result<Term, DomainError> {
        Ok(term)
    }
}

fn quad_to_internal_quad(quad: Quad) -> Result<InternalQuad<Term>, DomainError> {
    let subject = match quad.subject {
        SubjectTerm::Iri(iri) => Term::NamedNode(NamedNode::new_unchecked(iri.as_str())),
        SubjectTerm::BlankNode(id) => Term::BlankNode(BlankNode::new_unchecked(id)),
    };
    let predicate = Term::NamedNode(NamedNode::new_unchecked(quad.predicate.as_str()));
    let object = match quad.object {
        ObjectTerm::Iri(iri) => Term::NamedNode(NamedNode::new_unchecked(iri.as_str())),
        ObjectTerm::BlankNode(id) => Term::BlankNode(BlankNode::new_unchecked(id)),
        ObjectTerm::Literal {
            value,
            datatype,
            language,
        } => {
            let literal = if let Some(lang) = language {
                Literal::new_language_tagged_literal_unchecked(value, lang)
            } else {
                Literal::new_typed_literal(value, NamedNode::new_unchecked(datatype.as_str()))
            };
            Term::Literal(literal)
        }
    };
    let graph_name = quad
        .graph_iri
        .map(|g| Term::NamedNode(NamedNode::new_unchecked(g.as_str())));
    Ok(InternalQuad {
        subject,
        predicate,
        object,
        graph_name,
    })
}
