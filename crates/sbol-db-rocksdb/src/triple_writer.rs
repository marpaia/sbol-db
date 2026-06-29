//! Transactional [`TripleWriter`] for SPARQL Update over Oxigraph: every change
//! in the batch commits in one Oxigraph transaction, and each touched named
//! graph's accelerator index is marked stale in the companion afterward.
//!
//! The accelerator dirty-mark is a companion write outside the Oxigraph
//! transaction. It is idempotent and only ever over-marks (forcing a redundant
//! rebuild), so a crash between the two never serves stale accelerator data.

use std::collections::HashSet;

use async_trait::async_trait;
use oxigraph::store::Store;
use sbol_db_core::DomainError;
use sbol_db_storage::{TripleChange, TripleWriter, UpdateOutcome};

use crate::accel::AccelRepository;
use crate::convert::triple_to_quad;

#[derive(Clone)]
pub struct OxigraphTripleWriter {
    pub(crate) store: Store,
    pub(crate) accel: AccelRepository,
}

impl OxigraphTripleWriter {
    fn apply_blocking(&self, changes: &[TripleChange]) -> Result<UpdateOutcome, DomainError> {
        let mut outcome = UpdateOutcome::default();
        let mut txn = self
            .store
            .start_transaction()
            .map_err(|e| DomainError::Database(format!("oxigraph txn: {e}")))?;
        for change in changes {
            match change {
                TripleChange::Change { deletes, inserts } => {
                    for triple in deletes {
                        let quad = triple_to_quad(triple);
                        let before = txn
                            .quads_for_pattern(
                                Some(quad.subject.as_ref()),
                                Some(quad.predicate.as_ref()),
                                Some(quad.object.as_ref()),
                                Some(quad.graph_name.as_ref()),
                            )
                            .next()
                            .is_some();
                        if before {
                            txn.remove(quad.as_ref());
                            outcome.deleted += 1;
                        }
                    }
                    for triple in inserts {
                        let quad = triple_to_quad(triple);
                        let exists = txn
                            .quads_for_pattern(
                                Some(quad.subject.as_ref()),
                                Some(quad.predicate.as_ref()),
                                Some(quad.object.as_ref()),
                                Some(quad.graph_name.as_ref()),
                            )
                            .next()
                            .is_some();
                        if !exists {
                            txn.insert(quad.as_ref());
                            outcome.inserted += 1;
                        }
                    }
                }
                TripleChange::Clear(graph) => {
                    let graph_ref = match graph {
                        Some(iri) => oxrdf::GraphNameRef::NamedNode(
                            oxrdf::NamedNodeRef::new_unchecked(iri.as_str()),
                        ),
                        None => oxrdf::GraphNameRef::DefaultGraph,
                    };
                    let removed: Vec<_> = txn
                        .quads_for_pattern(None, None, None, Some(graph_ref))
                        .collect::<Result<_, _>>()
                        .map_err(|e| DomainError::Database(format!("oxigraph clear scan: {e}")))?;
                    for quad in removed {
                        txn.remove(quad.as_ref());
                        outcome.deleted += 1;
                    }
                }
            }
        }
        txn.commit()
            .map_err(|e| DomainError::Database(format!("oxigraph commit: {e}")))?;
        Ok(outcome)
    }
}

/// Every named graph an update touches; the default (graphless) partition is
/// never accelerated and is skipped.
fn touched_named_graphs(changes: &[TripleChange]) -> HashSet<String> {
    let mut graphs = HashSet::new();
    for change in changes {
        match change {
            TripleChange::Change { deletes, inserts } => {
                for triple in deletes.iter().chain(inserts) {
                    if let Some(graph) = &triple.graph_iri {
                        graphs.insert(graph.as_str().to_owned());
                    }
                }
            }
            TripleChange::Clear(graph) => {
                if let Some(graph) = graph {
                    graphs.insert(graph.as_str().to_owned());
                }
            }
        }
    }
    graphs
}

#[async_trait]
impl TripleWriter for OxigraphTripleWriter {
    async fn apply_update(&self, changes: Vec<TripleChange>) -> Result<UpdateOutcome, DomainError> {
        let this = self.clone();
        let dirty = touched_named_graphs(&changes);
        let outcome = tokio::task::spawn_blocking(move || this.apply_blocking(&changes))
            .await
            .map_err(|e| DomainError::Database(format!("oxigraph write task: {e}")))??;
        for graph in dirty {
            self.accel.mark_dirty_pool(&graph).await?;
        }
        Ok(outcome)
    }
}
