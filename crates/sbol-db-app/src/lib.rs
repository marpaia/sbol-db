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
mod admin_audit;
mod attachment;
mod audit;
mod auth;
mod blob;
mod collection;
mod config;
mod download;
mod edit;
mod federation;
mod maintenance;
pub mod memory;
mod mutation;
mod namespace;
mod object;
mod permission;
mod plugin;
mod review;
mod search;
mod sequence;
mod submission;

pub use acl::{AclService, PUBLIC_GRAPH};
pub use admin_audit::{AdminAuditEvent, AdminAuditOutcome, AdminAuditService};
pub use attachment::{
    attachment_uris, read_attachment, AttachmentRef, AttachmentService, UNKNOWN_ATTACHMENT_TYPE,
};
pub use audit::{AuditAction, AuditEvent, AuditService};
pub use auth::{share_hash, AuthService, PasswordReset, ProfileUpdate, Registration};
pub use blob::FsBlobStore;
pub use collection::{CollectionService, MintScope, MintedSubmission, Submission};
pub use config::{ConfigError, ConfigService};
pub use download::{Downloader, RemoteObjectResolver, DEFAULT_DATABASE_PREFIX};
pub use edit::{EditService, FieldValue};
pub use federation::{
    FederationError, FederationService, HttpWebOfRegistriesClient, JoinPayload, JoinResponse,
    WebOfRegistriesClient, WorInstance,
};
pub use maintenance::{MaintenanceScheduleReceipt, SearchMaintenanceScheduler};
pub use mutation::{MakePublicOutcome, MakePublicRequest, MutationError, MutationService};
pub use namespace::{
    RegistryNamespace, DEFAULT_DATABASE_PREFIX as DEFAULT_REGISTRY_DATABASE_PREFIX,
    DEFAULT_PUBLIC_GRAPH,
};
pub use object::{
    ObjectAttachment, ObjectAttachmentSection, ObjectContentState, ObjectDetails, ObjectProperty,
    ObjectPropertyValue, ObjectProvenance, ObjectReference, ObjectReferenceSection,
    ObjectSequenceContent, ObjectVisibility, ObjectVisualFeature, ObjectVisualGlyph,
    ObjectVisualization,
};
pub use permission::PermissionService;
pub use plugin::{
    CallPluginRequest, ExposeRegistry, HttpPluginClient, PluginClient, PluginError, PluginResponse,
    PluginService, StreamOutcome, StreamRegistry, StreamServe, PLUGIN_CATEGORIES,
};
pub use review::{ReviewCase, ReviewDecision, ReviewService, ReviewStatus};
pub use sbol_db_search::ranked_text::Hit;
pub use sbol_db_search::{AlignMode, AlignOptions};
pub use sbol_db_storage::SequenceAlignment;
pub use search::{
    DateField, DiscoveryFacetValue, DiscoveryFacets, DiscoveryHit, DiscoveryPage, DiscoveryQuery,
    DiscoverySort, FacetedSearch, LegacyExplorerStrategy, SortDirection,
};
pub use sequence::{SequenceService, SimilarHit};
pub use submission::{
    SubmissionService, SubmitConsequence, SubmitNotice, SubmitNoticeLevel, SubmitOutcome,
    SubmitPreview, SubmitRequest,
};

use std::sync::{Arc, OnceLock, RwLock};
use std::time::Instant;

use sbol_db_backend::Backend;
use sbol_db_core::DomainError;
use sbol_db_search::ranked_text::{ClusterMap, RankedTextIndex};
use sbol_db_search::{SearchDeployment, SearchRuntime, VectorRouter};
use sbol_db_search_sdk::{IndexMaintenanceEvent, IndexMaintenanceRegistry, IndexMutationSource};
use sbol_db_sparql::{SparqlEngine, SparqlUpdateEngine};
use sbol_db_storage::{
    AclStore, BlobStore, ClusterId, ClusterStore, ConfigStore, JobQueue, PageRankStore, SbolStore,
    SketchStore, TokenStore, TripleSource, UserStore,
};
use tokio::sync::Mutex as AsyncMutex;

type SharedClusterMapCache = Arc<RwLock<Option<(Instant, Arc<ClusterMap>)>>>;

/// Cache-aware view of a durable cluster store. Every mutation invalidates the
/// compact ranking map after the backend commit succeeds, while reads retain
/// the production-scale TTL/single-flight optimization.
struct InvalidatingClusterStore {
    inner: Arc<dyn ClusterStore>,
    cache: SharedClusterMapCache,
}

impl InvalidatingClusterStore {
    fn new(inner: Arc<dyn ClusterStore>, cache: SharedClusterMapCache) -> Self {
        Self { inner, cache }
    }

    fn invalidate(&self) {
        *self.cache.write().expect("cluster map cache lock poisoned") = None;
    }
}

#[async_trait::async_trait]
impl ClusterStore for InvalidatingClusterStore {
    async fn cluster_id_of(&self, iri: &str) -> Result<Option<ClusterId>, DomainError> {
        self.inner.cluster_id_of(iri).await
    }

    async fn cluster_mates(&self, iri: &str) -> Result<Vec<String>, DomainError> {
        self.inner.cluster_mates(iri).await
    }

    async fn replace_clusters(&self, pairs: Vec<(String, ClusterId)>) -> Result<(), DomainError> {
        self.inner.replace_clusters(pairs).await?;
        self.invalidate();
        Ok(())
    }

    async fn all_assignments(&self) -> Result<Vec<(String, ClusterId)>, DomainError> {
        self.inner.all_assignments().await
    }

    async fn assign_cluster(&self, iri: &str, cluster: ClusterId) -> Result<(), DomainError> {
        self.inner.assign_cluster(iri, cluster).await?;
        self.invalidate();
        Ok(())
    }

    async fn max_cluster_id(&self) -> Result<Option<ClusterId>, DomainError> {
        self.inner.max_cluster_id().await
    }
}

/// The application facade: the neutral trait objects plus the identity-aware
/// subsystems every HTTP adapter shares.
#[derive(Clone)]
pub struct AppServices {
    /// Immutable graph/object namespace for this registry. Production migration
    /// installs the source registry's HTTPS values before serving requests.
    pub registry_namespace: RegistryNamespace,
    /// The SBOL-aware store: ingest plus every derived-view read surface.
    pub store: Arc<dyn SbolStore>,
    /// SPARQL query evaluation over the derived triple view.
    pub sparql: Arc<SparqlEngine>,
    /// Exact triple-pattern reads for bounded application projections. This is
    /// the same source the SPARQL engine owns, exposed here so point-read paths
    /// do not rebuild simple relationships through the generic query engine.
    pub(crate) triple_source: Arc<dyn TripleSource>,
    /// SPARQL Update evaluation with transactional triple writes.
    pub sparql_update: Arc<SparqlUpdateEngine>,
    /// The async job queue.
    pub jobs: Arc<dyn JobQueue>,
    /// Content-addressed blob storage for attachment payloads, orthogonal to
    /// the triplestore. Defaults to a filesystem store under a temp directory;
    /// a deployment points it at a durable path with
    /// [`with_blobs`](Self::with_blobs).
    pub blobs: Arc<dyn BlobStore>,
    /// Ownership and sharing reads backing ACL-scoped queries.
    pub acl: Arc<dyn AclStore>,
    /// Object PageRank scores backing sequence-search and `/similar` ranking.
    /// The rebuild job writes it; the sequence facade reads it. Defaults to a
    /// non-persistent in-RAM store; a backend-built facade swaps in the durable
    /// one.
    pub pagerank: Arc<dyn PageRankStore>,
    /// Sequence cluster assignments backing `/similar`. The rebuild job writes
    /// it; the sequence facade reads it. Defaults to a non-persistent in-RAM
    /// store; a backend-built facade swaps in the durable one.
    pub cluster: Arc<dyn ClusterStore>,
    /// The MinHash/LSH similarity sketch index backing the sequence-search align
    /// path's candidate generation. The rebuild job writes it and the sequence
    /// facade reads it; the incremental write path updates a single sequence in
    /// place. Defaults to a non-persistent in-RAM store; a backend-built facade
    /// swaps in the durable one.
    pub sketch: Arc<dyn SketchStore>,
    /// Durable instance configuration (registries, remotes, plugins, mail,
    /// theme), the replacement for classic SynBioHub's `config.local.json`.
    /// Defaults to a non-persistent in-RAM store; a backend-built facade swaps
    /// in the durable one. The [`ConfigService`](Self::config_service) layers
    /// admin-gated mutation on top.
    pub config: Arc<dyn ConfigStore>,
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
    /// Short-lived ranking input cache. Loading the production
    /// cluster table on every search is much more expensive than scoring a
    /// small result page; the TTL refreshes maintenance changes promptly.
    pub(crate) cluster_map_cache: SharedClusterMapCache,
    /// Single-flight guard for an expired cluster cache. It prevents a burst of
    /// requests from all rescanning the production assignment table together.
    pub(crate) cluster_map_refresh: Arc<AsyncMutex<()>>,
    /// Lazily assembled structured-search runtime. The built-in runtime wraps
    /// the current text index and cluster store; an embedding/vector deployment
    /// can replace it with [`with_search_runtime`](Self::with_search_runtime).
    search_runtime: Arc<OnceLock<Arc<SearchRuntime>>>,
    /// Optional logical-index router used by embedding and hybrid strategies.
    /// The request path wraps it in the caller's graph authorization scope.
    search_vectors: Option<Arc<VectorRouter>>,
    /// Internal bridge from committed writes to plugin-defined durable search
    /// maintenance. The default registry is empty until a search deployment
    /// installs policies that opt an index into automatic maintenance.
    search_maintenance: Arc<SearchMaintenanceScheduler>,
    /// The Web of Registries HTTP client backing federation. Defaults to the
    /// SSRF-guarded [`HttpWebOfRegistriesClient`]; a test swaps in a stub with
    /// [`with_federation_client`](Self::with_federation_client). The
    /// [`FederationService`](Self::federation) pairs it with the config store.
    pub federation_client: Arc<dyn WebOfRegistriesClient>,
    /// The plugin HTTP client backing the `/callPlugin` proxy. Defaults to the
    /// SSRF-guarded [`HttpPluginClient`]; a test swaps in a stub with
    /// [`with_plugin_client`](Self::with_plugin_client). The
    /// [`PluginService`](Self::plugins) pairs it with the config store.
    pub plugin_client: Arc<dyn PluginClient>,
    /// The registry of time-limited exposed artifacts backing `/expose/:id`.
    pub expose: Arc<ExposeRegistry>,
    /// The async long-run handoff registry backing `/stream/:id`.
    pub stream: Arc<StreamRegistry>,
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
        .with_sequence_stores(
            backend.pagerank.clone(),
            backend.cluster.clone(),
            backend.sketch.clone(),
        )
        .with_config(backend.config.clone())
    }

    /// Replace the default in-RAM PageRank and cluster stores with durable
    /// ones, so the sequence facade ranks and answers `/similar` against the
    /// backend's own tables. [`from_backend`](Self::from_backend) applies this;
    /// a caller assembling the facade with [`new`](Self::new) may too.
    pub fn with_sequence_stores(
        mut self,
        pagerank: Arc<dyn PageRankStore>,
        cluster: Arc<dyn ClusterStore>,
        sketch: Arc<dyn SketchStore>,
    ) -> Self {
        let cluster_map_cache = Arc::new(RwLock::new(None));
        self.pagerank = pagerank;
        self.cluster = Arc::new(InvalidatingClusterStore::new(
            cluster,
            cluster_map_cache.clone(),
        ));
        self.sketch = sketch;
        self.cluster_map_cache = cluster_map_cache;
        self.cluster_map_refresh = Arc::new(AsyncMutex::new(()));
        // The built-in legacy strategy captures the cluster store. Reset the
        // lazy runtime so it observes the newly installed durable handle.
        self.search_runtime = Arc::new(OnceLock::new());
        self
    }

    /// Replace the default in-RAM config store with a caller-provided one,
    /// typically the backend's durable store, so instance configuration
    /// survives a restart. [`from_backend`](Self::from_backend) applies this.
    pub fn with_config(mut self, config: Arc<dyn ConfigStore>) -> Self {
        self.config = config;
        self
    }

    /// The admin-gated configuration facade over the durable config store.
    pub fn config_service(&self) -> ConfigService {
        ConfigService::new(self.config.clone())
    }

    /// The Web of Registries federation facade over the durable config store and
    /// the shared federation HTTP client. Also serves as the
    /// [`RemoteObjectResolver`] for cross-instance closure resolution.
    pub fn federation(&self) -> FederationService {
        FederationService::new(self.config.clone(), self.federation_client.clone())
    }

    /// Replace the default SSRF-guarded federation HTTP client with a
    /// caller-provided one, typically a stub in a test.
    pub fn with_federation_client(mut self, client: Arc<dyn WebOfRegistriesClient>) -> Self {
        self.federation_client = client;
        self
    }

    /// The plugin configuration and proxy facade over the durable config store
    /// and the shared plugin HTTP client.
    pub fn plugins(&self) -> PluginService {
        PluginService::new(self.config.clone(), self.plugin_client.clone())
    }

    /// Replace the default SSRF-guarded plugin HTTP client with a
    /// caller-provided one, typically a stub in a test.
    pub fn with_plugin_client(mut self, client: Arc<dyn PluginClient>) -> Self {
        self.plugin_client = client;
        self
    }

    /// The sequence-search and `/similar` facade over the SBOL store, the
    /// PageRank scores, and the cluster assignments.
    pub fn sequence(&self) -> SequenceService {
        SequenceService::new(
            self.store.clone(),
            self.pagerank.clone(),
            self.cluster.clone(),
            self.sketch.clone(),
        )
    }

    /// Replace the identity layer with explicit user and token stores,
    /// rebuilding the [`AuthService`] over them. Lets a caller that assembled
    /// the facade with [`new`](Self::new)'s default in-memory identity swap in
    /// a backend's durable stores, e.g. a conformance harness that constructs
    /// the neutral handles directly yet must exercise the real per-backend
    /// identity persistence.
    pub fn with_identity(mut self, users: Arc<dyn UserStore>, tokens: Arc<dyn TokenStore>) -> Self {
        self.auth = AuthService::new(users.clone(), tokens.clone())
            .with_database_prefix(self.registry_namespace.database_prefix());
        self.users = users;
        self.tokens = tokens;
        self
    }

    /// Install the registry's persistent namespace across identity, ACL, URI
    /// minting, mutation, visibility, and download consumers.
    pub fn with_registry_namespace(mut self, namespace: RegistryNamespace) -> Self {
        self.acl_service = AclService::new(self.store.clone(), self.acl.clone())
            .with_public_graph(namespace.public_graph());
        self.auth = AuthService::new(self.users.clone(), self.tokens.clone())
            .with_database_prefix(namespace.database_prefix());
        self.registry_namespace = namespace;
        self
    }

    /// Replace the in-RAM default ranked text index with a caller-provided one,
    /// typically the shared filesystem-backed index at a configured path. The
    /// index is shared with the rebuild job so both read and write the same
    /// corpus.
    pub fn with_text_search(mut self, text_search: Arc<RankedTextIndex>) -> Self {
        self.text_search = text_search;
        // The built-in legacy strategy captures the text index. Reset the lazy
        // runtime so it observes the replacement.
        self.search_runtime = Arc::new(OnceLock::new());
        self
    }

    /// Replace the built-in structured-search runtime with an explicitly
    /// assembled registry. Call this after installing low-level text/sequence
    /// handles so later builder steps do not intentionally reset the default.
    pub fn with_search_runtime(mut self, runtime: Arc<SearchRuntime>) -> Self {
        let slot = OnceLock::new();
        // A fresh slot cannot already be initialized.
        let _ = slot.set(runtime);
        self.search_runtime = Arc::new(slot);
        self
    }

    /// Install vector-index bindings for structured strategies. This does not
    /// alter the compatibility search endpoints or register a strategy by
    /// itself; deployments explicitly assemble both the strategy runtime and
    /// this router.
    pub fn with_vector_router(mut self, router: Arc<VectorRouter>) -> Self {
        self.search_vectors = Some(router);
        self
    }

    /// Install a topology that was validated once by the shared search
    /// deployment builder. Query dispatch and logical vector routing therefore
    /// cannot be assembled from different plugin configurations.
    pub fn with_search_deployment(self, deployment: &SearchDeployment) -> Self {
        let mut services = self
            .with_search_runtime(deployment.runtime())
            .with_vector_router(deployment.router());
        services.search_maintenance = Arc::new(SearchMaintenanceScheduler::new(
            services.jobs.clone(),
            deployment.maintenance(),
        ));
        services
    }

    /// Schedule plugin-defined maintenance after a committed write. Applications
    /// using a facade method such as [`Self::submission_service`] get this
    /// automatically; raw protocol adapters call it after their own commit.
    pub async fn schedule_search_maintenance(
        &self,
        event: IndexMaintenanceEvent,
    ) -> Result<MaintenanceScheduleReceipt, sbol_db_core::DomainError> {
        self.search_maintenance.schedule(event).await
    }

    /// Submit a corpus-level invalidation for an opaque mutation whose complete
    /// set of affected search documents cannot be known safely.
    pub async fn schedule_search_reconciliation(
        &self,
        source: IndexMutationSource,
    ) -> Result<MaintenanceScheduleReceipt, sbol_db_core::DomainError> {
        self.schedule_search_maintenance(IndexMaintenanceEvent::corpus(source))
            .await
    }

    /// The submission facade wired to automatic search maintenance, when the
    /// installed deployment opted any index into it.
    pub fn submission_service(&self) -> SubmissionService {
        SubmissionService::with_maintenance(
            self.store.clone(),
            CollectionService::new()
                .with_database_prefix(self.registry_namespace.database_prefix()),
            self.search_maintenance.clone(),
        )
    }

    /// The destructive-mutation facade wired to automatic search maintenance.
    pub fn mutation_service(&self) -> MutationService {
        MutationService::new(
            self.store.clone(),
            self.sparql_update.clone(),
            self.acl_service.clone(),
        )
        .with_database_prefix(self.registry_namespace.database_prefix())
        .with_maintenance(self.search_maintenance.clone())
    }

    /// The in-place edit facade wired to automatic search maintenance.
    pub fn edit_service(&self) -> EditService {
        EditService::new(
            self.store.clone(),
            self.sparql_update.clone(),
            self.acl_service.clone(),
        )
        .with_maintenance(self.search_maintenance.clone())
    }

    /// The ownership and read-only collaboration command facade.
    pub fn permission_service(&self) -> PermissionService {
        PermissionService::new(self.sparql_update.clone(), self.acl_service.clone())
    }

    /// The append-only object activity projection.
    pub fn audit_service(&self) -> AuditService {
        AuditService::new(self.sparql.clone())
    }

    /// The append-only administrator activity stream.
    pub fn admin_audit_service(&self) -> AdminAuditService {
        AdminAuditService::new(self.sparql.clone(), self.sparql_update.clone())
    }

    /// The curator review command and queue facade.
    pub fn review_service(&self) -> ReviewService {
        ReviewService::new(
            self.sparql.clone(),
            self.sparql_update.clone(),
            self.acl_service.clone(),
        )
    }

    /// The attachment facade wired to automatic search maintenance.
    pub fn attachment_service(&self) -> AttachmentService {
        AttachmentService::new(
            self.store.clone(),
            self.sparql_update.clone(),
            self.acl_service.clone(),
            self.blobs.clone(),
        )
        .with_maintenance(self.search_maintenance.clone())
    }

    /// Replace the default temp-directory blob store with a caller-provided one,
    /// typically a filesystem store rooted at a configured durable path.
    pub fn with_blobs(mut self, blobs: Arc<dyn BlobStore>) -> Self {
        self.blobs = blobs;
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
        let triple_source = sparql.triple_source();
        let acl_service = AclService::new(store.clone(), acl.clone());
        let auth = AuthService::new(users.clone(), tokens.clone());
        let text_search = Arc::new(
            RankedTextIndex::in_ram().expect("in-RAM ranked text index construction cannot fail"),
        );
        let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsBlobStore::new(
            std::env::temp_dir().join("sbol-db-blobs"),
        ));
        let pagerank: Arc<dyn PageRankStore> = Arc::new(memory::InMemoryPageRankStore::new());
        let cluster_map_cache = Arc::new(RwLock::new(None));
        let cluster: Arc<dyn ClusterStore> = Arc::new(InvalidatingClusterStore::new(
            Arc::new(memory::InMemoryClusterStore::new()),
            cluster_map_cache.clone(),
        ));
        let sketch: Arc<dyn SketchStore> = Arc::new(memory::InMemorySketchStore::new());
        let config: Arc<dyn ConfigStore> = Arc::new(memory::InMemoryConfigStore::new());
        let federation_client: Arc<dyn WebOfRegistriesClient> =
            Arc::new(federation::HttpWebOfRegistriesClient::new());
        let plugin_client: Arc<dyn PluginClient> = Arc::new(plugin::HttpPluginClient::new());
        let expose = Arc::new(plugin::ExposeRegistry::new());
        let stream = Arc::new(plugin::StreamRegistry::new());
        let search_runtime = Arc::new(OnceLock::new());
        let cluster_map_refresh = Arc::new(AsyncMutex::new(()));
        let search_maintenance = Arc::new(SearchMaintenanceScheduler::new(
            jobs.clone(),
            Arc::new(IndexMaintenanceRegistry::default()),
        ));
        Self {
            registry_namespace: RegistryNamespace::default(),
            store,
            sparql,
            triple_source,
            sparql_update,
            jobs,
            acl,
            pagerank,
            cluster,
            sketch,
            config,
            blobs,
            acl_service,
            users,
            tokens,
            auth,
            text_search,
            cluster_map_cache,
            cluster_map_refresh,
            search_runtime,
            search_vectors: None,
            search_maintenance,
            federation_client,
            plugin_client,
            expose,
            stream,
        }
    }
}
