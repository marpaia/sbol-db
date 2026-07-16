//! Caller-identity to graph-scope resolution.
//!
//! [`AclService`] reads the ownership and sharing facts already recorded in the
//! store (`sbh:ownedBy` / `sbh:canView`) and turns a caller into a
//! [`GraphScope`] the SPARQL engine enforces on reads. The store itself stays
//! authorization-free; only this service knows how an identity maps to the set
//! of graphs it may read.

use std::collections::BTreeSet;
use std::sync::Arc;

use sbol_db_core::DomainError;
use sbol_db_sparql::GraphScope;
use sbol_db_storage::{AclStore, SbolStore};

/// The public graph. Its contents are readable by everyone, authenticated or
/// not, so it is always present in a computed scope.
pub const PUBLIC_GRAPH: &str = "http://synbiohub.org/public";

/// Turns a caller identity into the [`GraphScope`] its reads are authorized
/// for.
#[derive(Clone)]
pub struct AclService {
    store: Arc<dyn SbolStore>,
    acl: Arc<dyn AclStore>,
}

impl AclService {
    pub fn new(store: Arc<dyn SbolStore>, acl: Arc<dyn AclStore>) -> Self {
        Self { store, acl }
    }

    /// The graph scope a caller may read.
    ///
    /// For an authenticated caller (`Some(user_graph_iri)`, the identity's RDF
    /// bridge `http://synbiohub.org/user/<username>`) the scope is
    /// [`GraphScope::Only`] of the always-readable public graph, the graphs the
    /// user owns (`sbh:ownedBy`), and the graphs holding the objects shared
    /// with the user (`sbh:canView`).
    ///
    /// For an anonymous caller (`None`) the scope is [`GraphScope::Union`],
    /// preserving the unauthenticated behavior the public endpoints have today.
    /// True anonymous restriction (scoping anon to the public graph alone) is a
    /// later phase; introducing it here would regress the existing public
    /// `/sparql` surface, so anon reads stay unrestricted for now.
    pub async fn compute_scope(
        &self,
        user_graph_iri: Option<&str>,
    ) -> Result<GraphScope, DomainError> {
        let Some(user) = user_graph_iri else {
            return Ok(GraphScope::Union);
        };

        let mut graphs: BTreeSet<String> = BTreeSet::new();
        graphs.insert(PUBLIC_GRAPH.to_owned());

        for owned in self.acl.owned_graphs(user).await? {
            graphs.insert(owned);
        }

        // A shared fact names an object, not its graph; resolve each shared
        // object to the graph that holds it so the scope stays graph-level.
        for object in self.acl.viewable_objects(user).await? {
            if let Some(graph) = self.graph_of_object(&object).await? {
                graphs.insert(graph);
            }
        }

        Ok(GraphScope::Only(graphs.into_iter().collect()))
    }

    /// The named graph holding `object_iri`, or `None` when the object is
    /// unknown or its graph carries no document IRI.
    async fn graph_of_object(&self, object_iri: &str) -> Result<Option<String>, DomainError> {
        let Some(record) = self.store.get_object_by_iri(object_iri).await? else {
            return Ok(None);
        };
        let Some(graph_id) = record.graph_id else {
            return Ok(None);
        };
        let Some(graph) = self.store.get_graph(graph_id).await? else {
            return Ok(None);
        };
        Ok(graph.document_iri.map(|iri| iri.into_inner()))
    }
}
