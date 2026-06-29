//! Postgres implementations of the `sbol-db-storage` contract.
//!
//! The async-trait impls delegate to the inherent methods on
//! [`SbolObjectService`] and [`JobRepository`]; inherent-method resolution
//! takes priority over the trait method of the same name, so these delegations
//! do not recurse. [`PostgresTripleSource`] and [`PostgresTripleWriter`] adapt
//! the async [`TripleRepository`] to the synchronous [`TripleSource`] and the
//! transactional [`TripleWriter`] contracts.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sbol_db_core::{
    DomainError, GraphId, GraphRecord, ImportReport, JobId, NeighborhoodQuery, NeighborhoodResult,
    ObjectId, SbolObjectRecord, SerializationFormat, Triple,
};
use sbol_db_storage::{
    AccelSolutions, AcceleratedQuery, BatchSequenceMatch, ClassCount, CorpusCounts, EnqueueOutcome,
    GraphFilter, GraphOverview, GraphStore, GraphTriplesPage, GraphWriteMode, ImportInput,
    JobAttempt, JobLogRecord, JobQueue, JobStatus, LabStore, ListGraphsFilter, ListJobsFilter,
    ListObjectsFilter, NeighborhoodStore, NewJob, ObjectStore, OldestQueuedAge, OntologyLoadReport,
    OntologyRecord, OntologyStore, OntologyTermRecord, PatternObject, PatternSubject,
    QueueDepthRow, SbolJob, SbolStore, SequenceMatch, SequenceSearchOptions, SequenceSearchStore,
    TripleChange, TripleSource, TripleWriter, UpdateOutcome,
};
use serde_json::Value;
use tokio::runtime::Handle;

use crate::repo::db_err;
use crate::{AccelRepository, JobRepository, PgPool, SbolObjectService, TripleRepository};

/// Verbatim source tag for triples written by SPARQL Update.
const UPDATE_SOURCE: &str = "sparql-update";

/// Synchronous [`TripleSource`] over the async [`TripleRepository`].
///
/// Each method runs the async scan to completion via
/// `Handle::current().block_on(...)`. This is only valid when called from
/// inside `tokio::task::spawn_blocking`, which is exactly how the SPARQL engine
/// drives the evaluator's synchronous `QueryableDataset`.
#[derive(Clone)]
pub struct PostgresTripleSource {
    triples: Arc<TripleRepository>,
    accel: AccelRepository,
}

impl PostgresTripleSource {
    pub fn new(triples: Arc<TripleRepository>, accel: AccelRepository) -> Self {
        Self { triples, accel }
    }
}

impl TripleSource for PostgresTripleSource {
    fn scan_pattern(
        &self,
        subject: Option<&PatternSubject>,
        predicate: Option<&str>,
        object: Option<&PatternObject>,
        graph: Option<&GraphFilter>,
        limit: i64,
    ) -> Result<Vec<Triple>, DomainError> {
        Handle::current().block_on(
            self.triples
                .scan_pattern(subject, predicate, object, graph, limit),
        )
    }

    fn distinct_named_graphs(&self) -> Result<Vec<String>, DomainError> {
        Handle::current().block_on(self.triples.distinct_named_graphs())
    }

    fn triples_for_graph(
        &self,
        graph: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Triple>, DomainError> {
        Handle::current().block_on(self.triples.triples_for_graph(graph, limit))
    }

    fn triples_for_subject(&self, subject_iri: &str) -> Result<Vec<Triple>, DomainError> {
        Handle::current().block_on(self.triples.triples_for_subject(subject_iri))
    }

    fn run_accelerated(
        &self,
        query: &AcceleratedQuery,
    ) -> Result<Option<AccelSolutions>, DomainError> {
        Handle::current().block_on(self.accel.run(query)).map(Some)
    }
}

/// Transactional [`TripleWriter`] for SPARQL Update.
///
/// `apply_update` runs the whole batch in one Postgres transaction: each
/// `Change` deletes, registers any named graph its inserts target (the graph
/// row owns its triples via FK), then inserts; each `Clear` drops a graph's
/// contents. Either the whole update commits or none of it does.
#[derive(Clone)]
pub struct PostgresTripleWriter {
    triples: Arc<TripleRepository>,
    pool: PgPool,
}

impl PostgresTripleWriter {
    pub fn new(triples: Arc<TripleRepository>, pool: PgPool) -> Self {
        Self { triples, pool }
    }
}

/// Every named graph an update touches (insert, delete, or clear), whose
/// accelerator indexes must be marked stale. The default (graphless) partition
/// has no named graph and is never accelerated, so it is skipped.
fn touched_named_graphs(changes: &[TripleChange]) -> std::collections::HashSet<String> {
    let mut graphs = std::collections::HashSet::new();
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
impl TripleWriter for PostgresTripleWriter {
    async fn apply_update(&self, changes: Vec<TripleChange>) -> Result<UpdateOutcome, DomainError> {
        let mut outcome = UpdateOutcome::default();
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        for change in &changes {
            match change {
                TripleChange::Change { deletes, inserts } => {
                    outcome.deleted += self.triples.delete_triples(&mut tx, deletes).await?;
                    let mut ensured = std::collections::HashSet::new();
                    for triple in inserts {
                        if let Some(graph) = &triple.graph_iri {
                            if ensured.insert(graph.as_str().to_owned()) {
                                self.triples
                                    .ensure_graph(&mut tx, graph.as_str(), "verbatim")
                                    .await?;
                            }
                        }
                    }
                    outcome.inserted += self
                        .triples
                        .insert_triples(&mut tx, inserts, UPDATE_SOURCE)
                        .await?;
                }
                TripleChange::Clear(graph) => {
                    outcome.deleted += self
                        .triples
                        .clear_graph(&mut tx, graph.as_ref().map(|i| i.as_str()))
                        .await?;
                }
            }
        }
        for graph in touched_named_graphs(&changes) {
            AccelRepository::mark_dirty(&mut tx, &graph).await?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(outcome)
    }
}

impl SbolObjectService {
    /// A [`TripleSource`] view of this service's triple store, for the SPARQL
    /// read engine.
    pub fn triple_source(&self) -> Arc<dyn TripleSource> {
        Arc::new(PostgresTripleSource::new(
            Arc::new(self.triples().clone()),
            self.accel().clone(),
        ))
    }

    /// A [`TripleWriter`] view of this service's triple store, for SPARQL Update.
    pub fn triple_writer(&self) -> Arc<dyn TripleWriter> {
        Arc::new(PostgresTripleWriter::new(
            Arc::new(self.triples().clone()),
            self.pool().clone(),
        ))
    }
}

#[async_trait]
impl ObjectStore for SbolObjectService {
    async fn get_object_by_iri(&self, iri: &str) -> Result<Option<SbolObjectRecord>, DomainError> {
        self.objects().get_by_iri(iri).await
    }

    async fn get_objects_by_iris(
        &self,
        iris: &[&str],
    ) -> Result<Vec<SbolObjectRecord>, DomainError> {
        self.objects().get_by_iris(iris).await
    }

    async fn list_objects(
        &self,
        filter: &ListObjectsFilter,
    ) -> Result<Vec<SbolObjectRecord>, DomainError> {
        self.objects().list(filter).await
    }

    async fn get_object_iri_by_id(&self, id: ObjectId) -> Result<Option<String>, DomainError> {
        self.objects().get_iri_by_id(id).await
    }
}

#[async_trait]
impl GraphStore for SbolObjectService {
    async fn get_graph(&self, id: GraphId) -> Result<Option<GraphRecord>, DomainError> {
        self.graphs().get(id).await
    }

    async fn list_graphs(
        &self,
        filter: &ListGraphsFilter,
    ) -> Result<Vec<GraphRecord>, DomainError> {
        self.graphs().list(filter).await
    }

    async fn delete_graph(&self, id: GraphId) -> Result<bool, DomainError> {
        self.graphs().delete(id).await
    }

    async fn graph_exists_by_hash(&self, hash: &[u8]) -> Result<bool, DomainError> {
        self.graphs().exists_by_hash(hash).await
    }
}

#[async_trait]
impl OntologyStore for SbolObjectService {
    async fn load_ontology_from_url(
        &self,
        prefix: &str,
        name: &str,
        source_url: &str,
    ) -> Result<OntologyLoadReport, DomainError> {
        self.ontology()
            .load_from_url(prefix, name, source_url)
            .await
    }

    async fn load_ontology_from_text(
        &self,
        prefix: &str,
        name: &str,
        source_url: Option<&str>,
        text: &str,
    ) -> Result<OntologyLoadReport, DomainError> {
        self.ontology()
            .load_from_text(prefix, name, source_url, text)
            .await
    }

    async fn list_ontologies(&self) -> Result<Vec<OntologyRecord>, DomainError> {
        self.ontology().list_ontologies().await
    }

    async fn canonicalize(&self, iri: &str) -> Result<Option<String>, DomainError> {
        self.ontology().canonicalize(iri).await
    }

    async fn descendants(&self, iri: &str) -> Result<Vec<(String, i16)>, DomainError> {
        self.ontology().descendants(iri).await
    }

    async fn list_ontology_terms(
        &self,
        prefix: &str,
        limit: i64,
        offset: i64,
        search: Option<&str>,
    ) -> Result<(Vec<OntologyTermRecord>, i64), DomainError> {
        self.ontology()
            .list_terms(prefix, limit, offset, search)
            .await
    }

    async fn get_ontology_term(
        &self,
        iri: &str,
    ) -> Result<Option<OntologyTermRecord>, DomainError> {
        self.ontology().get_term(iri).await
    }
}

#[async_trait]
impl LabStore for SbolObjectService {
    async fn corpus_counts(&self) -> Result<CorpusCounts, DomainError> {
        self.lab().corpus_counts().await
    }

    async fn recent_graphs(&self, limit: i64) -> Result<Vec<GraphOverview>, DomainError> {
        self.lab().list_graph_overviews(None, limit, 0).await
    }

    async fn top_classes(&self, limit: i64) -> Result<Vec<ClassCount>, DomainError> {
        self.lab().top_classes(limit).await
    }

    async fn count_graphs(&self, kind: Option<&str>) -> Result<i64, DomainError> {
        self.lab().count_graphs(kind).await
    }

    async fn list_graph_overviews(
        &self,
        kind: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<GraphOverview>, DomainError> {
        self.lab().list_graph_overviews(kind, limit, offset).await
    }

    async fn get_graph_overview(&self, id: GraphId) -> Result<Option<GraphOverview>, DomainError> {
        self.lab().get_graph_overview(id).await
    }

    async fn graph_triples(
        &self,
        id: GraphId,
        limit: i64,
        offset: i64,
    ) -> Result<Option<GraphTriplesPage>, DomainError> {
        self.lab().graph_triples(id, limit, offset).await
    }
}

#[async_trait]
impl NeighborhoodStore for SbolObjectService {
    async fn walk(&self, query: &NeighborhoodQuery) -> Result<NeighborhoodResult, DomainError> {
        self.neighborhood().walk(query).await
    }
}

#[async_trait]
impl SequenceSearchStore for SbolObjectService {
    async fn search(
        &self,
        pattern: &str,
        options: SequenceSearchOptions,
    ) -> Result<Vec<SequenceMatch>, DomainError> {
        self.sequence_search().search(pattern, options).await
    }

    async fn search_many(
        &self,
        patterns: &[String],
        options: SequenceSearchOptions,
    ) -> Result<Vec<BatchSequenceMatch>, DomainError> {
        self.sequence_search().search_many(patterns, options).await
    }
}

#[async_trait]
impl SbolStore for SbolObjectService {
    async fn import_document(&self, input: ImportInput) -> Result<ImportReport, DomainError> {
        self.import_document(input).await
    }

    async fn import_documents(
        &self,
        inputs: Vec<ImportInput>,
    ) -> Result<Vec<ImportReport>, DomainError> {
        self.import_documents(inputs).await
    }

    async fn graph_store_write(
        &self,
        graph: &str,
        body: &str,
        format: SerializationFormat,
        mode: GraphWriteMode,
    ) -> Result<usize, DomainError> {
        self.graph_store_write(graph, body, format, mode).await
    }

    async fn graph_store_clear(&self, graph: &str) -> Result<usize, DomainError> {
        self.graph_store_clear(graph).await
    }

    async fn graph_store_read(&self, graph: &str) -> Result<Vec<Triple>, DomainError> {
        self.graph_store_read(graph).await
    }

    async fn triples_for_subject(&self, subject_iri: &str) -> Result<Vec<Triple>, DomainError> {
        self.triples().triples_for_subject(subject_iri).await
    }

    async fn ping(&self) -> Result<(), DomainError> {
        self.ping().await
    }
}

#[async_trait]
impl JobQueue for JobRepository {
    async fn enqueue(&self, input: NewJob) -> Result<EnqueueOutcome, DomainError> {
        self.enqueue(input).await
    }

    async fn append_log(
        &self,
        job_id: JobId,
        attempt_no: Option<i32>,
        level: &str,
        message: &str,
        fields: Value,
    ) -> Result<JobLogRecord, DomainError> {
        self.append_log(job_id, attempt_no, level, message, fields)
            .await
    }

    async fn list_logs(
        &self,
        id: JobId,
        after_id: Option<i64>,
        limit: u32,
    ) -> Result<Vec<JobLogRecord>, DomainError> {
        self.list_logs(id, after_id, limit).await
    }

    async fn dequeue(
        &self,
        queues: &[String],
        worker_id: &str,
        lease: Duration,
    ) -> Result<Option<SbolJob>, DomainError> {
        self.dequeue(queues, worker_id, lease).await
    }

    async fn renew_lease(
        &self,
        job_id: JobId,
        worker_id: &str,
        lease: Duration,
    ) -> Result<bool, DomainError> {
        self.renew_lease(job_id, worker_id, lease).await
    }

    async fn mark_succeeded(
        &self,
        job_id: JobId,
        worker_id: &str,
        result: Option<Value>,
    ) -> Result<(), DomainError> {
        self.mark_succeeded(job_id, worker_id, result).await
    }

    async fn mark_failed(
        &self,
        job_id: JobId,
        worker_id: &str,
        error: &str,
    ) -> Result<JobStatus, DomainError> {
        self.mark_failed(job_id, worker_id, error).await
    }

    async fn reap_expired_leases(&self) -> Result<u64, DomainError> {
        self.reap_expired_leases().await
    }

    async fn get(&self, id: JobId) -> Result<Option<SbolJob>, DomainError> {
        self.get(id).await
    }

    async fn list_attempts(&self, id: JobId) -> Result<Vec<JobAttempt>, DomainError> {
        self.list_attempts(id).await
    }

    async fn list(&self, filter: &ListJobsFilter) -> Result<Vec<SbolJob>, DomainError> {
        self.list(filter).await
    }

    async fn cancel(&self, id: JobId) -> Result<bool, DomainError> {
        self.cancel(id).await
    }

    async fn current_status(&self, id: JobId) -> Result<Option<JobStatus>, DomainError> {
        self.current_status(id).await
    }

    async fn queue_depth_snapshot(&self) -> Result<Vec<QueueDepthRow>, DomainError> {
        self.queue_depth_snapshot().await
    }

    async fn oldest_queued_age(&self) -> Result<Vec<OldestQueuedAge>, DomainError> {
        self.oldest_queued_age().await
    }
}
