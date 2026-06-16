//! The storage contract: traits a persistence backend implements.
//!
//! Async traits use `#[async_trait]` so they are object-safe and can be held
//! as `Arc<dyn ...>`. [`TripleSource`] is deliberately synchronous: it backs
//! the SPARQL evaluator's synchronous `QueryableDataset`, and a backend that
//! needs async runs it to completion internally.

use std::time::Duration;

use async_trait::async_trait;
use sbol_db_core::{
    DomainError, GraphId, GraphRecord, ImportReport, JobId, NeighborhoodQuery, NeighborhoodResult,
    ObjectId, SbolObjectRecord, SerializationFormat, Triple,
};
use serde_json::Value;

use crate::{
    BatchSequenceMatch, EnqueueOutcome, GraphFilter, GraphWriteMode, ImportInput, JobAttempt,
    JobLogRecord, JobStatus, ListGraphsFilter, ListJobsFilter, ListObjectsFilter, NewJob,
    OldestQueuedAge, OntologyLoadReport, OntologyRecord, OntologyTermRecord, PatternObject,
    PatternSubject, QueueDepthRow, SbolJob, SequenceMatch, SequenceSearchOptions, TripleChange,
    UpdateOutcome,
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
}

/// The full SBOL-aware store: ingest plus every derived-view read surface.
#[async_trait]
pub trait SbolStore:
    ObjectStore + GraphStore + OntologyStore + NeighborhoodStore + SequenceSearchStore
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
}
