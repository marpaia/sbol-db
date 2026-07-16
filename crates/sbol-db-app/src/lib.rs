//! Backend-neutral application facade for sbol-db.
//!
//! [`AppServices`] bundles the neutral storage trait objects a
//! [`sbol_db_backend::Backend`] produces (the SBOL store, the SPARQL read and
//! update engines, the job queue) together with the identity-aware subsystems
//! the HTTP adapters share. Every adapter talks to the facade rather than to a
//! concrete backend, so the same application logic drives whichever backend a
//! connection string selects.
//!
//! [`AclService`] turns a caller identity into a [`GraphScope`] the SPARQL
//! engine enforces on reads; [`AuthService`] owns password and API-token
//! authentication over the identity stores.

mod acl;
mod auth;
pub mod memory;
mod search;

pub use acl::{AclService, PUBLIC_GRAPH};
pub use auth::{AuthService, PasswordReset, Registration};
pub use sbol_db_search::ranked_text::Hit;
pub use search::{DateField, FacetedSearch};

use std::sync::Arc;

use sbol_db_backend::Backend;
use sbol_db_search::ranked_text::RankedTextIndex;
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use sbol_db_storage::{AclStore, JobQueue, SbolStore, TokenStore, UserStore};

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
    /// Account persistence for the identity layer.
    pub users: Arc<dyn UserStore>,
    /// API-token persistence for the identity layer.
    pub tokens: Arc<dyn TokenStore>,
    /// Password and API-token authentication over the identity stores.
    pub auth: AuthService,
    /// The shared ranked text index backing native free-text search. The
    /// rebuild job writes it and the search adapters read it; a caller that
    /// needs a persistent, filesystem-backed index swaps one in with
    /// [`with_text_search`](Self::with_text_search).
    pub text_search: Arc<RankedTextIndex>,
}

impl AppServices {
    /// Assemble the facade from already-built trait objects and engines,
    /// provisioning a non-persistent in-memory identity layer. This constructor
    /// serves callers that hold the individual handles directly and do not
    /// exercise persistent identity; `from_backend` is the usual entry point
    /// and wires the backend's durable user and token stores.
    pub fn new(
        store: Arc<dyn SbolStore>,
        sparql: Arc<SparqlEngine>,
        sparql_update: Arc<SparqlUpdateEngine>,
        jobs: Arc<dyn JobQueue>,
        acl: Arc<dyn AclStore>,
    ) -> Self {
        Self::assemble(
            store,
            sparql,
            sparql_update,
            jobs,
            acl,
            Arc::new(memory::InMemoryUserStore::new()),
            Arc::new(memory::InMemoryTokenStore::new()),
        )
    }

    /// Build the facade from an open [`Backend`]. The SPARQL engines are
    /// constructed over the backend's triple views; the store, job queue, ACL
    /// reads, and identity stores are shared trait objects the backend already
    /// holds.
    pub fn from_backend(backend: &Backend) -> Self {
        let sparql = Arc::new(SparqlEngine::new(backend.triple_source.clone()));
        let sparql_update = Arc::new(SparqlUpdateEngine::new(
            backend.triple_source.clone(),
            backend.triple_writer.clone(),
        ));
        Self::assemble(
            backend.store.clone(),
            sparql,
            sparql_update,
            backend.jobs.clone(),
            backend.acl.clone(),
            backend.users.clone(),
            backend.tokens.clone(),
        )
    }

    /// Replace the identity layer with explicit user and token stores,
    /// rebuilding the [`AuthService`] over them. Lets a caller that assembled
    /// the facade with [`new`](Self::new)'s default in-memory identity swap in
    /// a backend's durable stores, e.g. a conformance harness that constructs
    /// the neutral handles directly yet must exercise the real per-backend
    /// identity persistence.
    pub fn with_identity(mut self, users: Arc<dyn UserStore>, tokens: Arc<dyn TokenStore>) -> Self {
        self.auth = AuthService::new(users.clone(), tokens.clone());
        self.users = users;
        self.tokens = tokens;
        self
    }

    /// Replace the in-RAM default ranked text index with a caller-provided one,
    /// typically the shared filesystem-backed index at a configured path. The
    /// index is shared with the rebuild job so both read and write the same
    /// corpus.
    pub fn with_text_search(mut self, text_search: Arc<RankedTextIndex>) -> Self {
        self.text_search = text_search;
        self
    }

    /// Derive the identity-aware subsystems from the neutral handles and bundle
    /// everything together.
    fn assemble(
        store: Arc<dyn SbolStore>,
        sparql: Arc<SparqlEngine>,
        sparql_update: Arc<SparqlUpdateEngine>,
        jobs: Arc<dyn JobQueue>,
        acl: Arc<dyn AclStore>,
        users: Arc<dyn UserStore>,
        tokens: Arc<dyn TokenStore>,
    ) -> Self {
        let acl_service = AclService::new(store.clone(), acl.clone());
        let auth = AuthService::new(users.clone(), tokens.clone());
        let text_search = Arc::new(
            RankedTextIndex::in_ram().expect("in-RAM ranked text index construction cannot fail"),
        );
        Self {
            store,
            sparql,
            sparql_update,
            jobs,
            acl,
            acl_service,
            users,
            tokens,
            auth,
            text_search,
        }
    }
}
