//! The SQLite-backed SBOL store: ingest plus the derived-view read surface,
//! the SPARQL read/write adapters, and the storage-trait implementations
//! (objects, graphs, ontology, neighborhood, sequence search, and the lab
//! dashboard). The job queue lives in [`crate::repo::job`].

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use sbol_db_core::{
    DomainError, GraphId, GraphRecord, ImportReport, IriString, NeighborhoodQuery,
    NeighborhoodResult, ObjectId, SbolObjectRecord, SerializationFormat, Triple,
};
use sbol_db_derive::{build_import_plan, to_rdf_format};
use sbol_db_rdf::rdf_graph_to_triples;
use sbol_db_storage::{
    AccelSolutions, AcceleratedQuery, BatchSequenceMatch, ClassCount, CorpusCounts, GraphFilter,
    GraphOverview, GraphStore, GraphTriplesPage, GraphWriteMode, ImportInput, LabStore,
    ListGraphsFilter, ListObjectsFilter, NeighborhoodStore, ObjectStore, OntologyLoadReport,
    OntologyRecord, OntologyStore, OntologyTermRecord, PatternObject, PatternSubject, SbolStore,
    SequenceMatch, SequenceSearchOptions, SequenceSearchStore, TripleChange, TripleSource,
    TripleWriter, UpdateOutcome,
};
use tokio::runtime::Handle;

use crate::pool::db_err;
use crate::repo::{
    AccelRepository, GraphRepository, LabRepository, OntologyRepository, SbolObjectRepository,
    SequenceSearchRepository, TripleRepository,
};
use crate::SqlitePool;

/// Per-call cap on a Graph Store `GET`, matching the Postgres backend.
const GRAPH_READ_LIMIT: i64 = 5_000_000;
const UPDATE_SOURCE: &str = "sparql-update";

/// The SQLite SBOL store. Cloneable; all clones share one connection pool.
#[derive(Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
    graphs: GraphRepository,
    objects: SbolObjectRepository,
    triples: TripleRepository,
    accel: AccelRepository,
    ontology: OntologyRepository,
    sequences: SequenceSearchRepository,
    lab: LabRepository,
}

impl SqliteStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            graphs: GraphRepository::new(pool.clone()),
            objects: SbolObjectRepository::new(pool.clone()),
            triples: TripleRepository::new(pool.clone()),
            accel: AccelRepository::new(pool.clone(), TripleRepository::new(pool.clone())),
            ontology: OntologyRepository::new(pool.clone()),
            sequences: SequenceSearchRepository::new(pool.clone()),
            lab: LabRepository::new(pool.clone()),
            pool,
        }
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn ping(&self) -> Result<(), DomainError> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(db_err)
    }

    pub fn triple_source(&self) -> Arc<dyn TripleSource> {
        Arc::new(SqliteTripleSource {
            triples: Arc::new(self.triples.clone()),
            accel: self.accel.clone(),
        })
    }

    pub fn triple_writer(&self) -> Arc<dyn TripleWriter> {
        Arc::new(SqliteTripleWriter {
            triples: Arc::new(self.triples.clone()),
            pool: self.pool.clone(),
        })
    }

    pub async fn import_document(&self, input: ImportInput) -> Result<ImportReport, DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let report = self.import_into_conn(&mut tx, input).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(report)
    }

    pub async fn import_documents(
        &self,
        inputs: Vec<ImportInput>,
    ) -> Result<Vec<ImportReport>, DomainError> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let mut reports = Vec::with_capacity(inputs.len());
        for input in inputs {
            reports.push(self.import_into_conn(&mut tx, input).await?);
        }
        tx.commit().await.map_err(db_err)?;
        Ok(reports)
    }

    async fn import_into_conn(
        &self,
        conn: &mut sqlx::SqliteConnection,
        input: ImportInput,
    ) -> Result<ImportReport, DomainError> {
        let plan = build_import_plan(&input)?;

        self.graphs
            .insert(&mut *conn, plan.graph_id, plan.new_graph)
            .await?;
        let triple_count = self
            .triples
            .insert_triples(&mut *conn, &plan.triples, "sbol")
            .await?;
        AccelRepository::mark_dirty(&mut *conn, plan.graph_iri.as_str()).await?;
        let object_count = plan.summaries.len();
        for summary in &plan.summaries {
            self.objects
                .upsert(&mut *conn, summary, Some(plan.graph_id))
                .await?;
        }

        // Persist sequence projections so sequence search has data + a k-mer
        // index. (Other typed projections are not yet materialized in SQLite.)
        for sequence in &plan.projections.sequences {
            self.sequences.upsert_sequence(&mut *conn, sequence).await?;
        }

        Ok(ImportReport {
            graph_id: plan.graph_id,
            object_count,
            triple_count,
            validation_status: plan.validation_status,
            validation_issue_count: plan.validation_issue_count,
        })
    }

    pub async fn graph_store_write(
        &self,
        graph: &str,
        body: &str,
        format: SerializationFormat,
        mode: GraphWriteMode,
    ) -> Result<usize, DomainError> {
        let rdf_format = to_rdf_format(format)?;
        let parsed = sbol_rdf::Graph::parse(body, rdf_format)
            .map_err(|e| DomainError::Parse(e.to_string()))?;
        let triples = rdf_graph_to_triples(&parsed, &IriString::unchecked(graph));

        let mut tx = self.pool.begin().await.map_err(db_err)?;
        self.triples
            .ensure_graph(&mut tx, graph, "verbatim")
            .await?;
        if mode == GraphWriteMode::Replace {
            self.triples.clear_graph(&mut tx, Some(graph)).await?;
        }
        let inserted = self
            .triples
            .insert_triples(&mut tx, &triples, "graph-store")
            .await?;
        AccelRepository::mark_dirty(&mut tx, graph).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(inserted)
    }

    pub async fn graph_store_clear(&self, graph: &str) -> Result<usize, DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let deleted = self.triples.clear_graph(&mut tx, Some(graph)).await?;
        self.triples.delete_graph(&mut tx, graph).await?;
        AccelRepository::mark_dirty(&mut tx, graph).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(deleted)
    }

    pub async fn graph_store_read(&self, graph: &str) -> Result<Vec<Triple>, DomainError> {
        self.triples
            .triples_for_graph(Some(graph), GRAPH_READ_LIMIT)
            .await
    }
}

/// Synchronous [`TripleSource`] over the async triplestore, mirroring the
/// Postgres adapter: each call blocks the current async task to completion, so
/// it is only valid inside `tokio::task::spawn_blocking` (how the SPARQL
/// evaluator drives it).
#[derive(Clone)]
struct SqliteTripleSource {
    triples: Arc<TripleRepository>,
    accel: AccelRepository,
}

impl TripleSource for SqliteTripleSource {
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

/// Transactional [`TripleWriter`] for SPARQL Update: the whole batch commits or
/// none of it does.
#[derive(Clone)]
struct SqliteTripleWriter {
    triples: Arc<TripleRepository>,
    pool: SqlitePool,
}

/// Every named graph an update touches (insert, delete, or clear), whose
/// accelerator indexes must be marked stale. The default (graphless) partition
/// has no named graph and is never accelerated, so it is skipped.
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
impl TripleWriter for SqliteTripleWriter {
    async fn apply_update(&self, changes: Vec<TripleChange>) -> Result<UpdateOutcome, DomainError> {
        let mut outcome = UpdateOutcome::default();
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        for change in &changes {
            match change {
                TripleChange::Change { deletes, inserts } => {
                    outcome.deleted += self.triples.delete_triples(&mut tx, deletes).await?;
                    let mut ensured = HashSet::new();
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

#[async_trait]
impl ObjectStore for SqliteStore {
    async fn get_object_by_iri(&self, iri: &str) -> Result<Option<SbolObjectRecord>, DomainError> {
        self.objects.get_by_iri(iri).await
    }

    async fn get_objects_by_iris(
        &self,
        iris: &[&str],
    ) -> Result<Vec<SbolObjectRecord>, DomainError> {
        self.objects.get_by_iris(iris).await
    }

    async fn list_objects(
        &self,
        filter: &ListObjectsFilter,
    ) -> Result<Vec<SbolObjectRecord>, DomainError> {
        self.objects.list(filter).await
    }

    async fn get_object_iri_by_id(&self, id: ObjectId) -> Result<Option<String>, DomainError> {
        self.objects.get_iri_by_id(id).await
    }
}

#[async_trait]
impl GraphStore for SqliteStore {
    async fn get_graph(&self, id: GraphId) -> Result<Option<GraphRecord>, DomainError> {
        self.graphs.get(id).await
    }

    async fn list_graphs(
        &self,
        filter: &ListGraphsFilter,
    ) -> Result<Vec<GraphRecord>, DomainError> {
        self.graphs.list(filter).await
    }

    async fn delete_graph(&self, id: GraphId) -> Result<bool, DomainError> {
        self.graphs.delete(id).await
    }

    async fn graph_exists_by_hash(&self, hash: &[u8]) -> Result<bool, DomainError> {
        self.graphs.exists_by_hash(hash).await
    }
}

#[async_trait]
impl OntologyStore for SqliteStore {
    async fn load_ontology_from_url(
        &self,
        prefix: &str,
        name: &str,
        source_url: &str,
    ) -> Result<OntologyLoadReport, DomainError> {
        let client = reqwest::Client::builder()
            .user_agent("sbol-db/0.1 (+https://github.com/marpaia/sbol-db)")
            .build()
            .map_err(|e| DomainError::InvalidInput(format!("reqwest client: {e}")))?;
        let body = client
            .get(source_url)
            .send()
            .await
            .map_err(|e| DomainError::InvalidInput(format!("fetch {source_url}: {e}")))?
            .error_for_status()
            .map_err(|e| DomainError::InvalidInput(format!("HTTP {source_url}: {e}")))?
            .text()
            .await
            .map_err(|e| DomainError::InvalidInput(format!("decode {source_url}: {e}")))?;
        self.ontology
            .load_from_text(prefix, name, Some(source_url), &body)
            .await
    }

    async fn load_ontology_from_text(
        &self,
        prefix: &str,
        name: &str,
        source_url: Option<&str>,
        text: &str,
    ) -> Result<OntologyLoadReport, DomainError> {
        self.ontology
            .load_from_text(prefix, name, source_url, text)
            .await
    }

    async fn list_ontologies(&self) -> Result<Vec<OntologyRecord>, DomainError> {
        self.ontology.list_ontologies().await
    }

    async fn canonicalize(&self, iri: &str) -> Result<Option<String>, DomainError> {
        self.ontology.canonicalize(iri).await
    }

    async fn descendants(&self, iri: &str) -> Result<Vec<(String, i16)>, DomainError> {
        self.ontology.descendants(iri).await
    }

    async fn list_ontology_terms(
        &self,
        prefix: &str,
        limit: i64,
        offset: i64,
        search: Option<&str>,
    ) -> Result<(Vec<OntologyTermRecord>, i64), DomainError> {
        self.ontology
            .list_terms(prefix, limit, offset, search)
            .await
    }

    async fn get_ontology_term(
        &self,
        iri: &str,
    ) -> Result<Option<OntologyTermRecord>, DomainError> {
        self.ontology.get_term(iri).await
    }
}

#[async_trait]
impl NeighborhoodStore for SqliteStore {
    async fn walk(&self, query: &NeighborhoodQuery) -> Result<NeighborhoodResult, DomainError> {
        crate::repo::neighborhood::walk(&self.triples, &self.objects, query).await
    }
}

#[async_trait]
impl SequenceSearchStore for SqliteStore {
    async fn search(
        &self,
        pattern: &str,
        options: SequenceSearchOptions,
    ) -> Result<Vec<SequenceMatch>, DomainError> {
        self.sequences.search(pattern, options).await
    }

    async fn search_many(
        &self,
        patterns: &[String],
        options: SequenceSearchOptions,
    ) -> Result<Vec<BatchSequenceMatch>, DomainError> {
        self.sequences.search_many(patterns, options).await
    }
}

#[async_trait]
impl LabStore for SqliteStore {
    async fn corpus_counts(&self) -> Result<CorpusCounts, DomainError> {
        self.lab.corpus_counts().await
    }

    async fn recent_graphs(&self, limit: i64) -> Result<Vec<GraphOverview>, DomainError> {
        self.lab.list_graph_overviews(None, limit, 0).await
    }

    async fn top_classes(&self, limit: i64) -> Result<Vec<ClassCount>, DomainError> {
        self.lab.top_classes(limit).await
    }

    async fn count_graphs(&self, kind: Option<&str>) -> Result<i64, DomainError> {
        self.lab.count_graphs(kind).await
    }

    async fn list_graph_overviews(
        &self,
        kind: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<GraphOverview>, DomainError> {
        self.lab.list_graph_overviews(kind, limit, offset).await
    }

    async fn get_graph_overview(&self, id: GraphId) -> Result<Option<GraphOverview>, DomainError> {
        self.lab.get_graph_overview(id).await
    }

    async fn graph_triples(
        &self,
        id: GraphId,
        limit: i64,
        offset: i64,
    ) -> Result<Option<GraphTriplesPage>, DomainError> {
        self.lab.graph_triples(id, limit, offset).await
    }
}

#[async_trait]
impl SbolStore for SqliteStore {
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
        self.triples.triples_for_subject(subject_iri).await
    }

    async fn ping(&self) -> Result<(), DomainError> {
        self.ping().await
    }
}
