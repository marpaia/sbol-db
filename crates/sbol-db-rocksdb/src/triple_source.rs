//! Synchronous [`TripleSource`] over the Oxigraph store, for the SPARQL
//! evaluator's accelerator/neighborhood reads. Oxigraph is synchronous, so each
//! scan calls `quads_for_pattern` directly; the accelerator delegate is the
//! async SQLite companion, driven to completion on the current runtime (these
//! calls run inside `spawn_blocking`).
//!
//! `supports_id_scan` is `false`: the native Oxigraph engine answers SPARQL
//! itself (see [`crate::sparql`]), so the workspace's id-native dataset path is
//! never used here. This source backs only the accelerator rebuild and the
//! neighborhood walk.

use oxigraph::store::Store;
use sbol_db_core::{DomainError, Triple};
use sbol_db_storage::{
    AccelSolutions, AcceleratedQuery, GraphFilter, PatternObject, PatternSubject, TripleSource,
};
use tokio::runtime::Handle;

use crate::accel::AccelRepository;
use crate::convert::{graph_name_ref, graph_node, object_term, predicate_node, quad_to_triple};

#[derive(Clone)]
pub struct OxigraphTripleSource {
    pub(crate) store: Store,
    pub(crate) accel: AccelRepository,
}

impl OxigraphTripleSource {
    fn scan(
        &self,
        subject: Option<&PatternSubject>,
        predicate: Option<&str>,
        object: Option<&PatternObject>,
        graph: Option<&GraphFilter>,
        limit: i64,
    ) -> Result<Vec<Triple>, DomainError> {
        let subject_node = subject.map(crate::convert::subject_node);
        let predicate_node = predicate.map(predicate_node);
        let object_term = object.map(object_term);
        let graph_node = graph.and_then(graph_node);

        let subject_ref = subject_node.as_ref().map(|s| s.as_ref());
        let predicate_ref = predicate_node.as_ref().map(|p| p.as_ref());
        let object_ref = object_term.as_ref().map(|o| o.as_ref());
        let graph_ref = graph_name_ref(graph, graph_node.as_ref());

        // `AnyNamed` has no single-bound expression, so it is matched by
        // post-filtering out default-graph quads.
        let drop_default = matches!(graph, Some(GraphFilter::AnyNamed));

        let cap = if limit < 0 {
            usize::MAX
        } else {
            limit as usize
        };
        let mut out = Vec::new();
        for quad in self
            .store
            .quads_for_pattern(subject_ref, predicate_ref, object_ref, graph_ref)
        {
            let quad = quad.map_err(|e| DomainError::Database(format!("oxigraph scan: {e}")))?;
            if drop_default && quad.graph_name == oxrdf::GraphName::DefaultGraph {
                continue;
            }
            out.push(quad_to_triple(&quad)?);
            if out.len() >= cap {
                break;
            }
        }
        Ok(out)
    }
}

impl TripleSource for OxigraphTripleSource {
    fn scan_pattern(
        &self,
        subject: Option<&PatternSubject>,
        predicate: Option<&str>,
        object: Option<&PatternObject>,
        graph: Option<&GraphFilter>,
        limit: i64,
    ) -> Result<Vec<Triple>, DomainError> {
        self.scan(subject, predicate, object, graph, limit)
    }

    fn distinct_named_graphs(&self) -> Result<Vec<String>, DomainError> {
        let mut graphs = Vec::new();
        for g in self.store.named_graphs() {
            let g = g.map_err(|e| DomainError::Database(format!("oxigraph named graphs: {e}")))?;
            match g {
                oxrdf::NamedOrBlankNode::NamedNode(n) => graphs.push(n.as_str().to_owned()),
                oxrdf::NamedOrBlankNode::BlankNode(_) => {}
            }
        }
        Ok(graphs)
    }

    fn triples_for_graph(
        &self,
        graph: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Triple>, DomainError> {
        let filter = match graph {
            Some(g) => GraphFilter::Iri(g.to_owned()),
            None => GraphFilter::DefaultOnly,
        };
        self.scan(None, None, None, Some(&filter), limit)
    }

    fn triples_for_subject(&self, subject_iri: &str) -> Result<Vec<Triple>, DomainError> {
        let subject = PatternSubject::Iri(subject_iri.to_owned());
        self.scan(Some(&subject), None, None, None, i64::MAX)
    }

    fn run_accelerated(
        &self,
        query: &AcceleratedQuery,
    ) -> Result<Option<AccelSolutions>, DomainError> {
        Handle::current().block_on(self.accel.run(query)).map(Some)
    }
}
