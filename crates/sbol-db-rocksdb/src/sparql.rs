//! The native SPARQL evaluator for the Oxigraph backend.
//!
//! Recognized SynBioHub templates short-circuit through the accelerator in the
//! workspace engine before this is reached. Everything else is evaluated by
//! Oxigraph's own id-native engine against the persistent store, at full
//! strength and without the workspace's `NOT EXISTS`→`MINUS` rewrite. The
//! result is serialized through the shared `sbol-db-sparql` writers, so the
//! bytes match every other backend.

use oxigraph::sparql::{QueryResults, SparqlEvaluator};
use oxigraph::store::Store;
use oxrdf::{GraphName, NamedNode};
use sbol_db_sparql::{
    serialize_boolean, serialize_solutions, serialize_triples, NativeSparql, ResultFormat,
    ResultPayload, SparqlError,
};

#[derive(Clone)]
pub struct OxigraphNativeSparql {
    store: Store,
}

impl OxigraphNativeSparql {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

impl NativeSparql for OxigraphNativeSparql {
    fn evaluate(
        &self,
        query: &spargebra::Query,
        format: ResultFormat,
        max_rows: usize,
        default_graph: Option<&str>,
    ) -> Result<ResultPayload, SparqlError> {
        let mut prepared = SparqlEvaluator::new().for_query(query.clone());
        // Dataset selection precedence mirrors the workspace engine:
        //   1. The query's own `FROM`/`FROM NAMED` wins (left untouched).
        //   2. Else the protocol `default-graph-uri` scopes the default graph.
        //   3. Else the default graph is the union of all named graphs (our
        //      writers put every triple in a named graph).
        if query.dataset().is_none() {
            match default_graph {
                Some(g) => prepared
                    .dataset_mut()
                    .set_default_graph(vec![GraphName::NamedNode(NamedNode::new_unchecked(g))]),
                None => prepared.dataset_mut().set_default_graph_as_union(),
            }
        }

        let results = prepared
            .on_store(&self.store)
            .execute()
            .map_err(|e| SparqlError::Evaluation(e.to_string()))?;
        match results {
            QueryResults::Solutions(iter) => serialize_solutions(iter, format, max_rows),
            QueryResults::Boolean(b) => serialize_boolean(b, format),
            QueryResults::Graph(iter) => serialize_triples(iter.into_iter(), format, max_rows),
        }
    }
}
