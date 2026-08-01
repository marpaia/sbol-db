//! The storage contract: traits a persistence backend implements.
//!
//! Async traits use `#[async_trait]` so they are object-safe and can be held
//! as `Arc<dyn ...>`. [`TripleSource`] is deliberately synchronous: it backs
//! the SPARQL evaluator's synchronous `QueryableDataset`, and a backend that
//! needs async runs it to completion internally.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use sbol_db_core::{
    BlobRef, ConfigEntry, DomainError, GraphId, GraphRecord, ImportReport, JobId,
    NeighborhoodQuery, NeighborhoodResult, NewUser, OAuthAccessToken, OAuthAuthorizationCode,
    OAuthClient, OAuthRefreshToken, ObjectId, ObjectTerm, SbolObjectRecord, SerializationFormat,
    Triple, User, UserId,
};
use sbol_db_search::{ClusterId, Signature};
use serde_json::Value;

use crate::{
    AccelSolutions, AcceleratedQuery, BatchSequenceMatch, EnqueueOutcome, GraphFilter,
    GraphWriteMode, IdGraphFilter, IdQuad, ImportInput, JobAttempt, JobLogRecord, JobStatus,
    ListGraphsFilter, ListJobsFilter, ListObjectsFilter, NewJob, OldestQueuedAge,
    OntologyLoadReport, OntologyRecord, OntologyTermRecord, PatternObject, PatternSubject,
    QueueDepthRow, RankRow, SbolJob, SequenceMatch, SequenceSearchOptions, TermId, TermKey,
    TermValue, TextSearchQuery, TripleChange, UpdateOutcome,
};

/// Synchronous triple-pattern reads, as required by the SPARQL evaluator.
pub trait TripleSource: Send + Sync {
    /// Scan triples matching a pattern. Any position may be bound or wildcarded
    /// (`None`); `limit` caps the rows returned per call.
    fn scan_pattern(
        &self,
        subject: Option<&PatternSubject>,
        predicate: Option<&str>,
        object: Option<&PatternObject>,
        graph: Option<&GraphFilter>,
        limit: i64,
    ) -> Result<Vec<Triple>, DomainError>;

    /// Distinct named graphs present in the store.
    fn distinct_named_graphs(&self) -> Result<Vec<String>, DomainError>;

    /// Every triple in one named graph (`Some`) or the default partition
    /// (`None`), capped at `limit`.
    fn triples_for_graph(
        &self,
        graph: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Triple>, DomainError>;

    /// Every triple with the given subject IRI.
    fn triples_for_subject(&self, subject_iri: &str) -> Result<Vec<Triple>, DomainError>;

    /// Whether this backend can scan by term id ([`Self::id_scan`]). An id-native
    /// backend lets the SPARQL evaluator join on term ids and materialize terms
    /// only for output rows; the default is `false` and the evaluator uses the
    /// term-materializing [`Self::scan_pattern`] path.
    fn supports_id_scan(&self) -> bool {
        false
    }

    /// Scan a pattern returning term ids rather than materialized triples. Bound
    /// positions are given as ids (see [`Self::term_to_id`]). Only meaningful when
    /// [`Self::supports_id_scan`] is true.
    fn id_scan(
        &self,
        _subject: Option<TermId>,
        _predicate: Option<TermId>,
        _object: Option<TermId>,
        _graph: &IdGraphFilter,
        _limit: i64,
    ) -> Result<Vec<IdQuad>, DomainError> {
        Err(DomainError::Database(
            "id_scan is not supported by this backend".into(),
        ))
    }

    /// Resolve a term to its id (the same id stored in the indexes), for binding
    /// query constants into an id-native scan.
    fn term_to_id(&self, _key: TermKey<'_>) -> Result<TermId, DomainError> {
        Err(DomainError::Database(
            "term_to_id is not supported by this backend".into(),
        ))
    }

    /// Resolve a term id back to its value, for materializing output rows and
    /// filter operands.
    fn id_to_term(&self, _id: TermId) -> Result<TermValue, DomainError> {
        Err(DomainError::Database(
            "id_to_term is not supported by this backend".into(),
        ))
    }

    /// Answer a recognized SynBioHub query from purpose-built indexes. Returns
    /// `Ok(None)` when this backend has no accelerator or declines the query, so
    /// the caller falls back to generic SPARQL evaluation.
    fn run_accelerated(
        &self,
        _query: &AcceleratedQuery,
    ) -> Result<Option<AccelSolutions>, DomainError> {
        Ok(None)
    }
}

/// Atomic batch application of SPARQL-update changes.
#[async_trait]
pub trait TripleWriter: Send + Sync {
    /// Apply every change in one atomic unit, registering any named graph an
    /// insert targets before writing it. Returns the inserted/deleted tally.
    async fn apply_update(&self, changes: Vec<TripleChange>) -> Result<UpdateOutcome, DomainError>;
}

/// Derived-view object reads.
#[async_trait]
pub trait ObjectStore: Send + Sync {
    async fn get_object_by_iri(&self, iri: &str) -> Result<Option<SbolObjectRecord>, DomainError>;
    async fn get_objects_by_iris(
        &self,
        iris: &[&str],
    ) -> Result<Vec<SbolObjectRecord>, DomainError>;
    async fn list_objects(
        &self,
        filter: &ListObjectsFilter,
    ) -> Result<Vec<SbolObjectRecord>, DomainError>;
    async fn get_object_iri_by_id(&self, id: ObjectId) -> Result<Option<String>, DomainError>;
}

/// Document-graph reads and deletion.
#[async_trait]
pub trait GraphStore: Send + Sync {
    async fn get_graph(&self, id: GraphId) -> Result<Option<GraphRecord>, DomainError>;
    async fn list_graphs(&self, filter: &ListGraphsFilter)
        -> Result<Vec<GraphRecord>, DomainError>;
    async fn delete_graph(&self, id: GraphId) -> Result<bool, DomainError>;
    async fn graph_exists_by_hash(&self, hash: &[u8]) -> Result<bool, DomainError>;
    /// Resolve a document graph's surrogate id from its document IRI, for
    /// delete-by-IRI. Returns `None` when no `sbol3`-kind graph carries it.
    async fn graph_id_by_document_iri(
        &self,
        document_iri: &str,
    ) -> Result<Option<GraphId>, DomainError>;
}

/// Ontology loading and lookup.
#[async_trait]
pub trait OntologyStore: Send + Sync {
    async fn load_ontology_from_url(
        &self,
        prefix: &str,
        name: &str,
        source_url: &str,
    ) -> Result<OntologyLoadReport, DomainError>;
    async fn load_ontology_from_text(
        &self,
        prefix: &str,
        name: &str,
        source_url: Option<&str>,
        text: &str,
    ) -> Result<OntologyLoadReport, DomainError>;
    async fn list_ontologies(&self) -> Result<Vec<OntologyRecord>, DomainError>;
    async fn canonicalize(&self, iri: &str) -> Result<Option<String>, DomainError>;
    async fn descendants(&self, iri: &str) -> Result<Vec<(String, i16)>, DomainError>;
    async fn list_ontology_terms(
        &self,
        prefix: &str,
        limit: i64,
        offset: i64,
        search: Option<&str>,
    ) -> Result<(Vec<OntologyTermRecord>, i64), DomainError>;
    async fn get_ontology_term(&self, iri: &str)
        -> Result<Option<OntologyTermRecord>, DomainError>;
}

/// Graph-neighborhood traversal.
#[async_trait]
pub trait NeighborhoodStore: Send + Sync {
    async fn walk(&self, query: &NeighborhoodQuery) -> Result<NeighborhoodResult, DomainError>;
}

/// Nucleotide sequence search over the derived view.
#[async_trait]
pub trait SequenceSearchStore: Send + Sync {
    async fn search(
        &self,
        pattern: &str,
        options: SequenceSearchOptions,
    ) -> Result<Vec<SequenceMatch>, DomainError>;
    async fn search_many(
        &self,
        patterns: &[String],
        options: SequenceSearchOptions,
    ) -> Result<Vec<BatchSequenceMatch>, DomainError>;

    /// Candidate `(sequence_iri, elements)` pairs for the banded aligner: every
    /// indexed DNA/RNA sequence sharing at least one canonical k-mer with
    /// `query` (on either strand, since the seeds are canonical). This is the
    /// k-mer prefilter feeding the alignment verify step in the application
    /// facade; the store gathers candidates and never aligns, so rust-bio stays
    /// out of the backends. A query shorter than the k-mer width has no seed, so
    /// every indexed nucleotide sequence is returned as a candidate.
    ///
    /// The similarity-search align path draws its candidates from the MinHash/LSH
    /// [`SketchStore`] instead; this k-mer prefilter remains the fallback for a
    /// query too short to sketch.
    async fn align_candidates(&self, query: &str) -> Result<Vec<(String, String)>, DomainError>;

    /// The `(sequence_iri, elements)` for the DNA/RNA sequences among `iris`,
    /// dropping any IRI that is absent or non-nucleotide. This materializes the
    /// elements of the [`SketchStore`] candidate set so the application facade
    /// can align them; order is unspecified.
    async fn sequences_by_iris(
        &self,
        iris: &[String],
    ) -> Result<Vec<(String, String)>, DomainError>;

    /// Every DNA/RNA `(sequence_iri, elements)` in the derived view, the full set
    /// the search-index rebuild sketches into the [`SketchStore`]. Keyed by the
    /// Sequence object's IRI, the same key the align path materializes elements
    /// by, so the sketch index and the align candidates stay aligned regardless
    /// of the source SBOL version. Order is unspecified.
    async fn all_nucleotide_sequences(&self) -> Result<Vec<(String, String)>, DomainError>;
}

/// Persistence for sequence cluster assignments backing `/similar`.
///
/// Clustering (greedy centroid, `vsearch --cluster_fast --id 0.8`) runs in the
/// search layer; this store persists the resulting `(sequence, cluster)`
/// assignments. The search-index rebuild recomputes every assignment and calls
/// [`replace_clusters`](Self::replace_clusters) to swap the whole table in one
/// transaction, so a read never sees a partial clustering. Answering `/similar`
/// at query time needs only this store and [`PageRankStore`]: no aligner, hence
/// no rust-bio, in the backends.
#[async_trait]
pub trait ClusterStore: Send + Sync {
    /// The cluster a sequence IRI belongs to, or `None` when it is unclustered.
    async fn cluster_id_of(&self, iri: &str) -> Result<Option<ClusterId>, DomainError>;

    /// The other members of `iri`'s cluster, excluding `iri` itself. Empty when
    /// `iri` is unclustered or the sole member. This is the `/similar` candidate
    /// set, which the caller then ranks by PageRank. Order is unspecified.
    async fn cluster_mates(&self, iri: &str) -> Result<Vec<String>, DomainError>;

    /// Replace every cluster assignment with `pairs` in one transaction: after
    /// it returns the table reflects exactly these `(sequence_iri, cluster_id)`
    /// pairs and nothing prior.
    async fn replace_clusters(&self, pairs: Vec<(String, ClusterId)>) -> Result<(), DomainError>;

    /// Every persisted `(sequence_iri, cluster_id)` assignment, a full scan of
    /// the cluster table. The ranked-text path builds its cluster-duplicate map
    /// from these, so a non-centroid cluster member is demoted in free-text
    /// search. Order is unspecified.
    async fn all_assignments(&self) -> Result<Vec<(String, ClusterId)>, DomainError>;

    /// Assign one sequence to a cluster, upserting: a later assignment for the
    /// same IRI overwrites the earlier one. This is the incremental write path a
    /// single new sequence takes, sparing the whole-table
    /// [`replace_clusters`](Self::replace_clusters) a full rebuild runs.
    ///
    /// The default reads every assignment, splices in `(iri, cluster)`, and
    /// writes them all back through [`replace_clusters`](Self::replace_clusters);
    /// a backend with a native upsert overrides it.
    async fn assign_cluster(&self, iri: &str, cluster: ClusterId) -> Result<(), DomainError> {
        let mut all = self.all_assignments().await?;
        all.retain(|(existing, _)| existing != iri);
        all.push((iri.to_owned(), cluster));
        self.replace_clusters(all).await
    }

    /// The largest cluster id in use, or `None` when no sequence is clustered.
    /// The incremental assign path opens a fresh cluster at one past this id.
    ///
    /// The default scans every assignment; a backend with an aggregate query
    /// overrides it.
    async fn max_cluster_id(&self) -> Result<Option<ClusterId>, DomainError> {
        Ok(self
            .all_assignments()
            .await?
            .into_iter()
            .map(|(_, cluster)| cluster)
            .max())
    }
}

/// Persistence for the MinHash/LSH similarity sketch index.
///
/// The sketch and its band hashes are computed in the search layer
/// ([`sbol_db_search::minhash`]); this store persists a sequence's
/// [`Signature`] and the LSH band buckets it falls into, and serves the
/// candidate-generation query. Candidate generation is a posting-list union
/// over band hashes ([`candidates_by_bands`](Self::candidates_by_bands)): the
/// sequences sharing at least one band with a query, which the banded aligner
/// then verifies. The store never sketches or aligns, so the algorithm stays in
/// the pure search crate and rust-bio stays out of the backends.
#[async_trait]
pub trait SketchStore: Send + Sync {
    /// Persist `iri`'s signature and its band buckets, replacing any prior
    /// sketch and bands for that IRI so a re-index leaves no stale postings.
    async fn put_sketch(
        &self,
        iri: &str,
        signature: &Signature,
        bands: &[u64],
    ) -> Result<(), DomainError>;

    /// The stored signature for `iri`, or `None` when it is unsketched.
    async fn sketch_of(&self, iri: &str) -> Result<Option<Signature>, DomainError>;

    /// The distinct sequence IRIs sharing at least one of `bands`, the LSH
    /// candidate set for a query with those band hashes. Order is unspecified.
    async fn candidates_by_bands(&self, bands: &[u64]) -> Result<Vec<String>, DomainError>;

    /// Every stored `(sequence_iri, signature)` pair, for a full rebuild. Order
    /// is unspecified.
    async fn all_sketches(&self) -> Result<Vec<(String, Signature)>, DomainError>;

    /// Replace every stored sketch and band posting with `entries` in one
    /// transaction: after it returns the index reflects exactly these
    /// `(sequence_iri, signature, band_hashes)` and nothing prior. The
    /// search-index rebuild swaps the whole index in through this, the sketch
    /// counterpart of [`ClusterStore::replace_clusters`] and
    /// [`PageRankStore::replace_all_ranks`], so a rebuild never leaves a stale
    /// posting behind.
    async fn replace_all_sketches(
        &self,
        entries: Vec<(String, Signature, Vec<u64>)>,
    ) -> Result<(), DomainError>;
}

/// Substring search over the derived object view.
#[async_trait]
pub trait TextSearchStore: Send + Sync {
    /// Search objects by substring, returning one page plus the total match
    /// count. A `limit` of 0 returns an empty page with the total only, so a
    /// caller wanting just the count pays for no row materialization.
    async fn search_objects(
        &self,
        query: &TextSearchQuery,
    ) -> Result<(Vec<SbolObjectRecord>, i64), DomainError>;
}

/// Persistence for object PageRank scores backing the native ranked search.
///
/// The search-index rebuild recomputes every score and calls
/// [`replace_all_ranks`](Self::replace_all_ranks) to swap the whole table
/// atomically; readers point-look-up a score or scan them all to feed the text
/// index. The store itself is ranking-free: it persists what the rebuild
/// computes and returns it unchanged.
#[async_trait]
pub trait PageRankStore: Send + Sync {
    /// The stored PageRank score for one object IRI, or `None` when the IRI
    /// carries no score. The combine step reads a missing score as `1.0`,
    /// SBOLExplorer's unknown-part convention.
    async fn rank_of(&self, iri: &str) -> Result<Option<f64>, DomainError>;

    /// The stored scores for a set of IRIs, keyed by IRI. IRIs with no stored
    /// score are absent from the map rather than defaulted, so the caller can
    /// tell "unranked" from a real score.
    async fn ranks_for(&self, iris: &[String]) -> Result<HashMap<String, f64>, DomainError>;

    /// Every stored `(iri, score)` pair, feeding a full text-index rebuild.
    async fn all_ranks(&self) -> Result<Vec<RankRow>, DomainError>;

    /// Replace the entire rank table with `ranks` in one transaction: after it
    /// returns the table reflects exactly this write and nothing prior.
    async fn replace_all_ranks(&self, ranks: Vec<RankRow>) -> Result<(), DomainError>;
}

/// The SynBioHub terms namespace, whose `ownedBy`/`canView` predicates carry
/// the ownership and sharing facts an [`AclStore`] reads.
pub const SBH_OWNED_BY: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#ownedBy";
/// The SynBioHub `canView` predicate: `<owner> sbh:canView <object>`.
pub const SBH_CAN_VIEW: &str = "http://wiki.synbiohub.org/wiki/Terms/synbiohub#canView";

/// Ownership and sharing reads backing ACL-scoped queries.
///
/// SynBioHub records visibility as triples: a graph's contents carry
/// `<subject> sbh:ownedBy <owner>` for each owner, and a shared object is
/// reachable through `<owner> sbh:canView <object>`. These reads let the app's
/// ACL layer turn a caller identity into the graphs and objects it may read;
/// the store itself stays authorization-free.
#[async_trait]
pub trait AclStore: Send + Sync {
    /// Named graphs whose contents carry `<subject> sbh:ownedBy <owner_iri>`,
    /// i.e. the graphs `owner_iri` owns and may read. Distinct, order
    /// unspecified.
    async fn owned_graphs(&self, owner_iri: &str) -> Result<Vec<String>, DomainError>;

    /// Objects shared with `owner_iri` through `<owner_iri> sbh:canView
    /// <object>`. Distinct, order unspecified.
    async fn viewable_objects(&self, owner_iri: &str) -> Result<Vec<String>, DomainError>;
}

/// Distinct named-graph IRIs among `triples`, dropping any in the default
/// partition. Backs [`AclStore::owned_graphs`] once a backend has scanned the
/// `sbh:ownedBy` triples.
pub fn distinct_graph_iris(triples: Vec<Triple>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    for t in triples {
        if let Some(g) = t.graph_iri {
            seen.insert(g.into_inner());
        }
    }
    seen.into_iter().collect()
}

/// Distinct object-position IRIs among `triples`. Backs
/// [`AclStore::viewable_objects`] once a backend has scanned the `sbh:canView`
/// triples.
pub fn distinct_object_iris(triples: Vec<Triple>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    for t in triples {
        if let ObjectTerm::Iri(o) = t.object {
            seen.insert(o.into_inner());
        }
    }
    seen.into_iter().collect()
}

/// Account persistence for the identity layer.
///
/// A separate trait object from [`SbolStore`]: the SBOL store stays
/// authorization-free while the application facade holds this alongside it.
/// Implementations map rows to [`User`] and enforce the unique `username` and
/// `email` constraints. Password hashing lives in the application layer; these
/// methods store and read the already-computed `password_hash`.
#[async_trait]
pub trait UserStore: Send + Sync {
    /// Create an account, assigning a fresh [`UserId`], and return it.
    async fn create_user(&self, new_user: NewUser) -> Result<User, DomainError>;

    /// Resolve an account by an identifier matching either its `email` or its
    /// `username`, mirroring SynBioHub's login lookup. `None` when neither
    /// matches.
    async fn find_by_email_or_username(
        &self,
        identifier: &str,
    ) -> Result<Option<User>, DomainError>;

    /// Fetch an account by id. `None` when no such account exists.
    async fn get_by_id(&self, id: UserId) -> Result<Option<User>, DomainError>;

    /// Every account ordered by username. Administrator projections use this
    /// instead of backend-specific SQL or column-family scans.
    async fn list_users(&self) -> Result<Vec<User>, DomainError>;

    /// Persist the mutable profile fields (`name`, `affiliation`, and the
    /// membership flags) of `user`, returning the stored account.
    async fn update_user(&self, user: &User) -> Result<User, DomainError>;

    /// Replace an account's stored password hash, e.g. transparent rehashing of
    /// a legacy digest to argon2 on a successful login.
    async fn set_password_hash(&self, id: UserId, password_hash: &str) -> Result<(), DomainError>;

    /// Set (`Some`) or clear (`None`) an account's outstanding reset link.
    async fn set_reset_link(&self, id: UserId, link: Option<&str>) -> Result<(), DomainError>;

    /// Atomically claim the account whose outstanding reset link equals `link`,
    /// clearing the link so it cannot be reused. Returns the claimed account, or
    /// `None` when no account carries that link.
    async fn consume_reset_link(&self, link: &str) -> Result<Option<User>, DomainError>;

    /// Delete the account with `id`, returning whether a row was removed
    /// (`false` when no such account exists).
    async fn delete_user(&self, id: UserId) -> Result<bool, DomainError>;

    /// Whether any administrator account exists. First-launch setup uses this to
    /// decide whether an instance still needs its initial administrator.
    async fn any_admin(&self) -> Result<bool, DomainError>;
}

/// API-token persistence for the identity layer.
///
/// Tokens are stored only as their hash; the application layer computes the
/// hash (sha3 of the plaintext token) so a persisted row cannot be replayed.
/// A separate trait object from [`SbolStore`], held by the application facade.
#[async_trait]
pub trait TokenStore: Send + Sync {
    /// Persist a token hash for `user_id`, associating the token with the
    /// account it authenticates.
    async fn issue(&self, token_hash: &str, user_id: UserId) -> Result<(), DomainError>;

    /// Resolve a token hash to the account it authenticates, or `None` when no
    /// live token carries that hash.
    async fn resolve(&self, token_hash: &str) -> Result<Option<UserId>, DomainError>;

    /// Revoke the token with the given hash, returning whether a token was
    /// removed.
    async fn revoke(&self, token_hash: &str) -> Result<bool, DomainError>;
}

/// Durable OAuth client and grant persistence for SBOL Identity.
///
/// All secret-bearing keys are one-way hashes computed by the application
/// layer. Code and refresh-token reads are destructive so they remain
/// single-use even when multiple server processes share a backend.
#[async_trait]
pub trait OAuthStore: Send + Sync {
    /// Register a public OAuth client. Re-registering an existing client id is
    /// rejected by the concrete store.
    async fn register_client(&self, client: OAuthClient) -> Result<(), DomainError>;

    /// Look up a client by its exact client id.
    async fn get_client(&self, client_id: &str) -> Result<Option<OAuthClient>, DomainError>;

    /// Persist a short-lived authorization code grant.
    async fn issue_authorization_code(
        &self,
        code: OAuthAuthorizationCode,
    ) -> Result<(), DomainError>;

    /// Atomically remove and return an authorization code grant.
    async fn consume_authorization_code(
        &self,
        code_hash: &str,
    ) -> Result<Option<OAuthAuthorizationCode>, DomainError>;

    /// Persist an audience-bound access token.
    async fn issue_access_token(&self, token: OAuthAccessToken) -> Result<(), DomainError>;

    /// Resolve a live access-token hash. Expiry is enforced by the application
    /// service so every backend uses identical clock semantics.
    async fn resolve_access_token(
        &self,
        token_hash: &str,
    ) -> Result<Option<OAuthAccessToken>, DomainError>;

    /// Revoke an access token by hash.
    async fn revoke_access_token(&self, token_hash: &str) -> Result<bool, DomainError>;

    /// Persist a rotating refresh token.
    async fn issue_refresh_token(&self, token: OAuthRefreshToken) -> Result<(), DomainError>;

    /// Atomically remove and return a refresh token during rotation.
    async fn consume_refresh_token(
        &self,
        token_hash: &str,
    ) -> Result<Option<OAuthRefreshToken>, DomainError>;
}

/// The full SBOL-aware store: ingest plus every derived-view read surface.
#[async_trait]
pub trait SbolStore:
    ObjectStore + GraphStore + OntologyStore + NeighborhoodStore + SequenceSearchStore + TextSearchStore
{
    async fn import_document(&self, input: ImportInput) -> Result<ImportReport, DomainError>;
    async fn import_documents(
        &self,
        inputs: Vec<ImportInput>,
    ) -> Result<Vec<ImportReport>, DomainError>;
    async fn graph_store_write(
        &self,
        graph: &str,
        body: &str,
        format: SerializationFormat,
        mode: GraphWriteMode,
    ) -> Result<usize, DomainError>;
    async fn graph_store_clear(&self, graph: &str) -> Result<usize, DomainError>;
    async fn graph_store_read(&self, graph: &str) -> Result<Vec<Triple>, DomainError>;
    /// Every triple with the given subject IRI, for single-object RDF export.
    async fn triples_for_subject(&self, subject_iri: &str) -> Result<Vec<Triple>, DomainError>;
    async fn ping(&self) -> Result<(), DomainError>;
}

/// The job queue: enqueue, lease-based dequeue, lifecycle transitions, and the
/// operator/observability read surface.
#[async_trait]
pub trait JobQueue: Send + Sync {
    async fn enqueue(&self, input: NewJob) -> Result<EnqueueOutcome, DomainError>;
    async fn append_log(
        &self,
        job_id: JobId,
        attempt_no: Option<i32>,
        level: &str,
        message: &str,
        fields: Value,
    ) -> Result<JobLogRecord, DomainError>;
    async fn list_logs(
        &self,
        id: JobId,
        after_id: Option<i64>,
        limit: u32,
    ) -> Result<Vec<JobLogRecord>, DomainError>;
    async fn dequeue(
        &self,
        queues: &[String],
        worker_id: &str,
        lease: Duration,
    ) -> Result<Option<SbolJob>, DomainError>;
    async fn renew_lease(
        &self,
        job_id: JobId,
        worker_id: &str,
        lease: Duration,
    ) -> Result<bool, DomainError>;
    async fn mark_succeeded(
        &self,
        job_id: JobId,
        worker_id: &str,
        result: Option<Value>,
    ) -> Result<(), DomainError>;
    async fn mark_failed(
        &self,
        job_id: JobId,
        worker_id: &str,
        error: &str,
    ) -> Result<JobStatus, DomainError>;
    async fn reap_expired_leases(&self) -> Result<u64, DomainError>;
    async fn get(&self, id: JobId) -> Result<Option<SbolJob>, DomainError>;
    async fn list_attempts(&self, id: JobId) -> Result<Vec<JobAttempt>, DomainError>;
    async fn list(&self, filter: &ListJobsFilter) -> Result<Vec<SbolJob>, DomainError>;
    async fn cancel(&self, id: JobId) -> Result<bool, DomainError>;
    async fn current_status(&self, id: JobId) -> Result<Option<JobStatus>, DomainError>;
    async fn queue_depth_snapshot(&self) -> Result<Vec<QueueDepthRow>, DomainError>;
    async fn oldest_queued_age(&self) -> Result<Vec<OldestQueuedAge>, DomainError>;

    /// Count jobs that reached a terminal failure (`failed` or `dead`) in the
    /// last `within_hours`. Backed by [`Self::list`] so every backend reports
    /// the same number without backend-specific SQL; the dashboard's
    /// failures tile reads it.
    async fn recent_failure_count(&self, within_hours: i64) -> Result<i64, DomainError> {
        let since = chrono::Utc::now() - chrono::Duration::hours(within_hours.max(0));
        let mut total = 0i64;
        for status in [JobStatus::Failed, JobStatus::Dead] {
            let filter = ListJobsFilter {
                kind: None,
                status: Some(status),
                queue: None,
                correlation_id: None,
                since: Some(since),
                limit: 10_000,
            };
            total += self.list(&filter).await?.len() as i64;
        }
        Ok(total)
    }
}

/// Content-addressed blob storage for attachment payloads, orthogonal to the
/// triplestore and held separately in the application facade rather than
/// reached through [`SbolStore`]. The contract is content-addressing by the
/// SHA-1 of the *uncompressed* bytes with gzip at rest: a blob's identity is
/// that hash, so writing identical bytes twice yields the same [`BlobRef`] and
/// one stored object.
#[async_trait]
pub trait BlobStore: Send + Sync {
    /// Store `bytes`, returning the content address, uncompressed size, and
    /// sniffed media type. Idempotent: storing bytes already present is a no-op
    /// that returns the same [`BlobRef`].
    async fn put(&self, bytes: &[u8]) -> Result<BlobRef, DomainError>;

    /// Fetch the decompressed bytes for content address `sha1`, or `None` when
    /// no blob with that hash is stored.
    async fn get(&self, sha1: &str) -> Result<Option<Vec<u8>>, DomainError>;

    /// Fetch the raw gzip bytes for `sha1` so a download can stream them under
    /// `Content-Encoding: gzip` without recompressing, or `None` when absent.
    async fn get_gz(&self, sha1: &str) -> Result<Option<Vec<u8>>, DomainError>;

    /// Whether a blob with content address `sha1` is stored.
    async fn exists(&self, sha1: &str) -> Result<bool, DomainError>;
}

/// Durable instance configuration, a flat key to JSON-value store.
///
/// This is the persistent equivalent of classic SynBioHub's mutable
/// `config.local.json`: each section (registries, remotes, plugins, mail,
/// theme, and the like) lives under one stable key with an arbitrary JSON
/// value. Held separately in the application facade rather than reached through
/// [`SbolStore`], mirroring [`PageRankStore`] and [`ClusterStore`], so the SBOL
/// store stays free of application configuration. The application's
/// `ConfigService` layers typed accessors and admin-gated mutation on top.
#[async_trait]
pub trait ConfigStore: Send + Sync {
    /// The value stored under `key`, or `None` when the key has never been set.
    async fn get(&self, key: &str) -> Result<Option<Value>, DomainError>;

    /// Write `value` under `key`, upserting: a later write to the same key
    /// overwrites the earlier value and refreshes its `updated_at`.
    async fn set(&self, key: &str, value: &Value) -> Result<(), DomainError>;

    /// Every stored entry. Order is unspecified.
    async fn get_all(&self) -> Result<Vec<ConfigEntry>, DomainError>;

    /// Remove the entry under `key`. Deleting an absent key is a no-op.
    async fn delete(&self, key: &str) -> Result<(), DomainError>;
}

#[cfg(test)]
mod blob_store_object_safety {
    /// Proves [`BlobStore`](super::BlobStore) is object-safe, so the facade can
    /// hold it as `Arc<dyn BlobStore>`.
    fn _assert_obj_safe(_: &dyn super::BlobStore) {}

    /// Proves [`ConfigStore`](super::ConfigStore) is object-safe, so the facade
    /// can hold it as `Arc<dyn ConfigStore>`.
    fn _assert_config_obj_safe(_: &dyn super::ConfigStore) {}
}
