//! Backend-neutral application facade for sbol-db.
//!
//! [`AppServices`] bundles the neutral storage trait objects a
//! [`sbol_db_backend::Backend`] produces (the SBOL store, the SPARQL read and
//! update engines, the job queue) together with the identity-aware subsystems
//! the HTTP adapters share. Every adapter talks to the facade rather than to a
//! concrete backend, so the same application logic drives whichever backend a
//! connection string selects.
//!
//! The first identity-aware subsystem is [`AclService`], which turns a caller
//! identity into a [`GraphScope`] the SPARQL engine enforces on reads.

mod acl;

pub use acl::{AclService, PUBLIC_GRAPH};

use std::sync::Arc;

use sbol_db_backend::Backend;
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use sbol_db_storage::{AclStore, JobQueue, SbolStore};

/// The application facade: the neutral trait objects plus the identity-aware
/// subsystems every HTTP adapter shares.
#[derive(Clone)]
pub struct AppServices {
    /// The SBOL-aware store: ingest plus every derived-view read surface.
    pub store: Arc<dyn SbolStore>,
    /// SPARQL query evaluation over the derived triple view.
    pub sparql: Arc<SparqlEngine>,
    /// SPARQL Update evaluation with transactional triple writes.
    pub sparql_update: Arc<SparqlUpdateEngine>,
    /// The async job queue.
    pub jobs: Arc<dyn JobQueue>,
    /// Ownership and sharing reads backing ACL-scoped queries.
    pub acl: Arc<dyn AclStore>,
    /// Turns a caller identity into the graph scope a read is authorized for.
    pub acl_service: AclService,
}

impl AppServices {
    /// Assemble the facade from already-built trait objects and engines,
    /// deriving the [`AclService`] from the store and ACL reads. `from_backend`
    /// is the usual entry point; this constructor serves callers that hold the
    /// individual handles directly.
    pub fn new(
        store: Arc<dyn SbolStore>,
        sparql: Arc<SparqlEngine>,
        sparql_update: Arc<SparqlUpdateEngine>,
        jobs: Arc<dyn JobQueue>,
        acl: Arc<dyn AclStore>,
    ) -> Self {
        let acl_service = AclService::new(store.clone(), acl.clone());
        Self {
            store,
            sparql,
            sparql_update,
            jobs,
            acl,
            acl_service,
        }
    }

    /// Build the facade from an open [`Backend`]. The SPARQL engines are
    /// constructed over the backend's triple views; the store, job queue, and
    /// ACL reads are shared trait objects the backend already holds.
    pub fn from_backend(backend: &Backend) -> Self {
        let sparql = Arc::new(SparqlEngine::new(backend.triple_source.clone()));
        let sparql_update = Arc::new(SparqlUpdateEngine::new(
            backend.triple_source.clone(),
            backend.triple_writer.clone(),
        ));
        Self::new(
            backend.store.clone(),
            sparql,
            sparql_update,
            backend.jobs.clone(),
            backend.acl.clone(),
        )
    }
}
