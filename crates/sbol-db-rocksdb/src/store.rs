//! The RocksDB-backed SBOL store: ingest plus the derived-view read surface,
//! the SPARQL read/write adapters, and the storage-trait implementations. Each
//! high-level write composes one atomic [`WriteBatch`]; async trait methods run
//! their RocksDB work on a blocking thread, while [`TripleSource`] (already
//! synchronous, driven inside the SPARQL evaluator's blocking task) calls the
//! engine directly.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use rocksdb::WriteBatch;
use sbol_db_core::{
    DomainError, GraphId, GraphRecord, ImportReport, IriString, NeighborhoodQuery,
    NeighborhoodResult, ObjectId, SbolObjectRecord, SerializationFormat, Triple,
};
use sbol_db_derive::{build_import_plan, to_rdf_format};
use sbol_db_rdf::{rdf_graph_to_triples, GRAPH_IRI_PREFIX};
use sbol_db_storage::{
    AccelSolutions, AcceleratedQuery, BatchSequenceMatch, ClassCount, CorpusCounts, GraphFilter,
    GraphOverview, GraphStore, GraphTriplesPage, GraphWriteMode, IdGraphFilter, IdQuad,
    ImportInput, LabStore, ListGraphsFilter, ListObjectsFilter, NeighborhoodStore, ObjectStore,
    OntologyLoadReport, OntologyRecord, OntologyStore, OntologyTermRecord, PatternObject,
    PatternSubject, SbolStore, SequenceMatch, SequenceSearchOptions, SequenceSearchStore, TermId,
    TermKey, TermValue, TripleChange, TripleSource, TripleWriter, UpdateOutcome,
};

use crate::codec::Term;
use crate::db::Db;
use crate::repo::accel::AccelRepository;
use crate::repo::neighborhood;
use crate::repo::{
    GraphRepository, LabRepository, ObjectRepository, OntologyRepository, SequenceSearchRepository,
    TripleRepository,
};

/// Per-call cap on a Graph Store `GET`, matching the other backends.
const GRAPH_READ_LIMIT: i64 = 5_000_000;

/// The RocksDB SBOL store. Cloneable; all clones share one database handle.
#[derive(Clone)]
pub struct RocksdbStore {
    db: Db,
    graphs: GraphRepository,
    objects: ObjectRepository,
    triples: TripleRepository,
    ontology: OntologyRepository,
    sequences: SequenceSearchRepository,
    lab: LabRepository,
    accel: AccelRepository,
}

impl RocksdbStore {
    pub fn new(db: Db) -> Self {
        Self {
            graphs: GraphRepository::new(db.clone()),
            objects: ObjectRepository::new(db.clone()),
            triples: TripleRepository::new(db.clone()),
            ontology: OntologyRepository::new(db.clone()),
            sequences: SequenceSearchRepository::new(db.clone()),
            lab: LabRepository::new(db.clone()),
            accel: AccelRepository::new(db.clone(), TripleRepository::new(db.clone())),
            db,
        }
    }

    pub fn triple_source(&self) -> Arc<dyn TripleSource> {
        Arc::new(RocksdbTripleSource {
            triples: self.triples.clone(),
            accel: self.accel.clone(),
        })
    }

    pub fn triple_writer(&self) -> Arc<dyn TripleWriter> {
        Arc::new(RocksdbTripleWriter {
            triples: self.triples.clone(),
            db: self.db.clone(),
            accel: self.accel.clone(),
        })
    }

    fn stage_import(
        &self,
        batch: &mut WriteBatch,
        seen: &mut HashSet<Vec<u8>>,
        input: ImportInput,
    ) -> Result<ImportReport, DomainError> {
        let plan = build_import_plan(&input)?;
        self.graphs
            .stage_insert(batch, plan.graph_id, &plan.new_graph)?;
        let triple_count = self.triples.stage_insert(batch, seen, &plan.triples)?;
        self.accel.stage_mark_dirty(batch, plan.graph_iri.as_str());
        let object_count = plan.summaries.len();
        for summary in &plan.summaries {
            self.objects
                .stage_upsert(batch, summary, Some(plan.graph_id))?;
        }
        for sequence in &plan.projections.sequences {
            self.sequences.stage_upsert(batch, sequence)?;
        }
        Ok(ImportReport {
            graph_id: plan.graph_id,
            object_count,
            triple_count,
            validation_status: plan.validation_status,
            validation_issue_count: plan.validation_issue_count,
        })
    }
}

async fn blocking<T, F>(f: F) -> Result<T, DomainError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, DomainError> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| DomainError::Database(format!("rocksdb task panicked: {e}")))?
}

impl RocksdbStore {
    async fn import_document(&self, input: ImportInput) -> Result<ImportReport, DomainError> {
        let this = self.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            let mut seen = HashSet::new();
            let report = this.stage_import(&mut batch, &mut seen, input)?;
            this.db.write(batch)?;
            Ok(report)
        })
        .await
    }

    async fn import_documents(
        &self,
        inputs: Vec<ImportInput>,
    ) -> Result<Vec<ImportReport>, DomainError> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let this = self.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            let mut seen = HashSet::new();
            let mut reports = Vec::with_capacity(inputs.len());
            for input in inputs {
                reports.push(this.stage_import(&mut batch, &mut seen, input)?);
            }
            this.db.write(batch)?;
            Ok(reports)
        })
        .await
    }

    async fn graph_store_write(
        &self,
        graph: &str,
        body: &str,
        format: SerializationFormat,
        mode: GraphWriteMode,
    ) -> Result<usize, DomainError> {
        let this = self.clone();
        let graph = graph.to_owned();
        let body = body.to_owned();
        blocking(move || {
            let rdf_format = to_rdf_format(format)?;
            let parsed = sbol_rdf::Graph::parse(&body, rdf_format)
                .map_err(|e| DomainError::Parse(e.to_string()))?;
            let triples = rdf_graph_to_triples(&parsed, &IriString::unchecked(graph.clone()));

            let mut batch = WriteBatch::default();
            let mut seen = HashSet::new();
            if mode == GraphWriteMode::Replace {
                this.triples.stage_clear_graph(&mut batch, Some(&graph))?;
            }
            let inserted = this.triples.stage_insert(&mut batch, &mut seen, &triples)?;
            this.accel.stage_mark_dirty(&mut batch, &graph);
            this.db.write(batch)?;
            Ok(inserted)
        })
        .await
    }

    async fn graph_store_clear(&self, graph: &str) -> Result<usize, DomainError> {
        let this = self.clone();
        let graph = graph.to_owned();
        blocking(move || {
            let mut batch = WriteBatch::default();
            let deleted = this.triples.stage_clear_graph(&mut batch, Some(&graph))?;
            this.accel.stage_mark_dirty(&mut batch, &graph);
            this.db.write(batch)?;
            Ok(deleted)
        })
        .await
    }

    async fn graph_store_read(&self, graph: &str) -> Result<Vec<Triple>, DomainError> {
        self.triples
            .triples_for_graph(Some(graph), GRAPH_READ_LIMIT)
            .await
    }
}

/// Synchronous [`TripleSource`] over the engine, for the SPARQL evaluator's
/// blocking task. RocksDB is synchronous, so each call runs directly.
#[derive(Clone)]
struct RocksdbTripleSource {
    triples: TripleRepository,
    accel: AccelRepository,
}

impl TripleSource for RocksdbTripleSource {
    fn scan_pattern(
        &self,
        subject: Option<&PatternSubject>,
        predicate: Option<&str>,
        object: Option<&PatternObject>,
        graph: Option<&GraphFilter>,
        limit: i64,
    ) -> Result<Vec<Triple>, DomainError> {
        self.triples
            .scan_pattern(subject, predicate, object, graph, limit)
    }

    fn distinct_named_graphs(&self) -> Result<Vec<String>, DomainError> {
        self.triples.distinct_named_graphs_blocking()
    }

    fn triples_for_graph(
        &self,
        graph: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Triple>, DomainError> {
        let filter = match graph {
            Some(g) => GraphFilter::Iri(g.to_owned()),
            None => GraphFilter::DefaultOnly,
        };
        self.triples
            .scan_pattern(None, None, None, Some(&filter), limit)
    }

    fn triples_for_subject(&self, subject_iri: &str) -> Result<Vec<Triple>, DomainError> {
        let subject = PatternSubject::Iri(subject_iri.to_owned());
        self.triples
            .scan_pattern(Some(&subject), None, None, None, i64::MAX)
    }

    fn supports_id_scan(&self) -> bool {
        true
    }

    fn id_scan(
        &self,
        subject: Option<TermId>,
        predicate: Option<TermId>,
        object: Option<TermId>,
        graph: &IdGraphFilter,
        limit: i64,
    ) -> Result<Vec<IdQuad>, DomainError> {
        self.triples
            .id_scan(subject, predicate, object, graph, limit)
    }

    fn term_to_id(&self, key: TermKey<'_>) -> Result<TermId, DomainError> {
        Ok(TripleRepository::term_id(&key))
    }

    fn id_to_term(&self, id: TermId) -> Result<TermValue, DomainError> {
        self.triples.resolve_value(id)
    }

    fn run_accelerated(
        &self,
        query: &AcceleratedQuery,
    ) -> Result<Option<AccelSolutions>, DomainError> {
        self.accel.run(query).map(Some)
    }
}

/// Transactional [`TripleWriter`] for SPARQL Update: the whole batch commits or
/// none of it does.
#[derive(Clone)]
struct RocksdbTripleWriter {
    triples: TripleRepository,
    db: Db,
    accel: AccelRepository,
}

#[async_trait]
impl TripleWriter for RocksdbTripleWriter {
    async fn apply_update(&self, changes: Vec<TripleChange>) -> Result<UpdateOutcome, DomainError> {
        let triples = self.triples.clone();
        let db = self.db.clone();
        let accel = self.accel.clone();
        blocking(move || {
            let mut outcome = UpdateOutcome::default();
            let mut batch = WriteBatch::default();
            let mut seen = HashSet::new();
            let mut dirty: HashSet<String> = HashSet::new();
            for change in &changes {
                match change {
                    TripleChange::Change { deletes, inserts } => {
                        outcome.deleted += triples.stage_delete(&mut batch, deletes)?;
                        outcome.inserted += triples.stage_insert(&mut batch, &mut seen, inserts)?;
                        for t in deletes.iter().chain(inserts.iter()) {
                            if let Some(g) = &t.graph_iri {
                                dirty.insert(g.as_str().to_owned());
                            }
                        }
                    }
                    TripleChange::Clear(graph) => {
                        outcome.deleted += triples
                            .stage_clear_graph(&mut batch, graph.as_ref().map(|i| i.as_str()))?;
                        if let Some(g) = graph {
                            dirty.insert(g.as_str().to_owned());
                        }
                    }
                }
            }
            for graph in &dirty {
                accel.stage_mark_dirty(&mut batch, graph);
            }
            db.write(batch)?;
            Ok(outcome)
        })
        .await
    }
}

#[async_trait]
impl ObjectStore for RocksdbStore {
    async fn get_object_by_iri(&self, iri: &str) -> Result<Option<SbolObjectRecord>, DomainError> {
        let objects = self.objects.clone();
        let iri = iri.to_owned();
        blocking(move || objects.get_by_iri(&iri)).await
    }

    async fn get_objects_by_iris(
        &self,
        iris: &[&str],
    ) -> Result<Vec<SbolObjectRecord>, DomainError> {
        let objects = self.objects.clone();
        let owned: Vec<String> = iris.iter().map(|s| s.to_string()).collect();
        blocking(move || {
            let refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
            objects.get_by_iris(&refs)
        })
        .await
    }

    async fn list_objects(
        &self,
        filter: &ListObjectsFilter,
    ) -> Result<Vec<SbolObjectRecord>, DomainError> {
        let objects = self.objects.clone();
        let filter = filter.clone();
        blocking(move || objects.list(&filter)).await
    }

    async fn get_object_iri_by_id(&self, id: ObjectId) -> Result<Option<String>, DomainError> {
        let objects = self.objects.clone();
        blocking(move || objects.get_iri_by_id(id)).await
    }
}

#[async_trait]
impl GraphStore for RocksdbStore {
    async fn get_graph(&self, id: GraphId) -> Result<Option<GraphRecord>, DomainError> {
        let graphs = self.graphs.clone();
        blocking(move || graphs.get(id)).await
    }

    async fn list_graphs(
        &self,
        filter: &ListGraphsFilter,
    ) -> Result<Vec<GraphRecord>, DomainError> {
        let graphs = self.graphs.clone();
        let filter = filter.clone();
        blocking(move || graphs.list(&filter)).await
    }

    async fn delete_graph(&self, id: GraphId) -> Result<bool, DomainError> {
        let this = self.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            let Some(_record) = this.graphs.stage_delete(&mut batch, id)? else {
                return Ok(false);
            };
            let iri = format!("{GRAPH_IRI_PREFIX}{}", id.0);
            let gid = Term::named(&iri).id();
            this.triples.stage_delete_named_graph(&mut batch, gid)?;
            this.objects.stage_delete_for_graph(&mut batch, id)?;
            this.db.write(batch)?;
            Ok(true)
        })
        .await
    }

    async fn graph_exists_by_hash(&self, hash: &[u8]) -> Result<bool, DomainError> {
        let graphs = self.graphs.clone();
        let hash = hash.to_vec();
        blocking(move || graphs.exists_by_hash(&hash)).await
    }
}

#[async_trait]
impl OntologyStore for RocksdbStore {
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
        self.load_ontology_from_text(prefix, name, Some(source_url), &body)
            .await
    }

    async fn load_ontology_from_text(
        &self,
        prefix: &str,
        name: &str,
        source_url: Option<&str>,
        text: &str,
    ) -> Result<OntologyLoadReport, DomainError> {
        let ontology = self.ontology.clone();
        let prefix = prefix.to_owned();
        let name = name.to_owned();
        let source_url = source_url.map(|s| s.to_owned());
        let text = text.to_owned();
        blocking(move || ontology.load_from_text(&prefix, &name, source_url.as_deref(), &text))
            .await
    }

    async fn list_ontologies(&self) -> Result<Vec<OntologyRecord>, DomainError> {
        let ontology = self.ontology.clone();
        blocking(move || ontology.list_ontologies()).await
    }

    async fn canonicalize(&self, iri: &str) -> Result<Option<String>, DomainError> {
        let ontology = self.ontology.clone();
        let iri = iri.to_owned();
        blocking(move || ontology.canonicalize(&iri)).await
    }

    async fn descendants(&self, iri: &str) -> Result<Vec<(String, i16)>, DomainError> {
        let ontology = self.ontology.clone();
        let iri = iri.to_owned();
        blocking(move || ontology.descendants(&iri)).await
    }

    async fn list_ontology_terms(
        &self,
        prefix: &str,
        limit: i64,
        offset: i64,
        search: Option<&str>,
    ) -> Result<(Vec<OntologyTermRecord>, i64), DomainError> {
        let ontology = self.ontology.clone();
        let prefix = prefix.to_owned();
        let search = search.map(|s| s.to_owned());
        blocking(move || ontology.list_terms(&prefix, limit, offset, search.as_deref())).await
    }

    async fn get_ontology_term(
        &self,
        iri: &str,
    ) -> Result<Option<OntologyTermRecord>, DomainError> {
        let ontology = self.ontology.clone();
        let iri = iri.to_owned();
        blocking(move || ontology.get_term(&iri)).await
    }
}

#[async_trait]
impl NeighborhoodStore for RocksdbStore {
    async fn walk(&self, query: &NeighborhoodQuery) -> Result<NeighborhoodResult, DomainError> {
        let triples = self.triples.clone();
        let objects = self.objects.clone();
        let query = query.clone();
        blocking(move || neighborhood::walk(&triples, &objects, &query)).await
    }
}

#[async_trait]
impl SequenceSearchStore for RocksdbStore {
    async fn search(
        &self,
        pattern: &str,
        options: SequenceSearchOptions,
    ) -> Result<Vec<SequenceMatch>, DomainError> {
        let sequences = self.sequences.clone();
        let pattern = pattern.to_owned();
        blocking(move || sequences.search(&pattern, options)).await
    }

    async fn search_many(
        &self,
        patterns: &[String],
        options: SequenceSearchOptions,
    ) -> Result<Vec<BatchSequenceMatch>, DomainError> {
        let sequences = self.sequences.clone();
        let patterns = patterns.to_vec();
        blocking(move || sequences.search_many(&patterns, options)).await
    }
}

#[async_trait]
impl LabStore for RocksdbStore {
    async fn corpus_counts(&self) -> Result<CorpusCounts, DomainError> {
        let lab = self.lab.clone();
        blocking(move || lab.corpus_counts()).await
    }

    async fn recent_graphs(&self, limit: i64) -> Result<Vec<GraphOverview>, DomainError> {
        let lab = self.lab.clone();
        blocking(move || lab.list_graph_overviews(None, limit, 0)).await
    }

    async fn top_classes(&self, limit: i64) -> Result<Vec<ClassCount>, DomainError> {
        let lab = self.lab.clone();
        blocking(move || lab.top_classes(limit)).await
    }

    async fn count_graphs(&self, kind: Option<&str>) -> Result<i64, DomainError> {
        let lab = self.lab.clone();
        let kind = kind.map(|k| k.to_owned());
        blocking(move || lab.count_graphs(kind.as_deref())).await
    }

    async fn list_graph_overviews(
        &self,
        kind: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<GraphOverview>, DomainError> {
        let lab = self.lab.clone();
        let kind = kind.map(|k| k.to_owned());
        blocking(move || lab.list_graph_overviews(kind.as_deref(), limit, offset)).await
    }

    async fn get_graph_overview(&self, id: GraphId) -> Result<Option<GraphOverview>, DomainError> {
        let lab = self.lab.clone();
        blocking(move || lab.get_graph_overview(id)).await
    }

    async fn graph_triples(
        &self,
        id: GraphId,
        limit: i64,
        offset: i64,
    ) -> Result<Option<GraphTriplesPage>, DomainError> {
        let lab = self.lab.clone();
        blocking(move || lab.graph_triples(id, limit, offset)).await
    }
}

#[async_trait]
impl SbolStore for RocksdbStore {
    async fn import_document(&self, input: ImportInput) -> Result<ImportReport, DomainError> {
        RocksdbStore::import_document(self, input).await
    }

    async fn import_documents(
        &self,
        inputs: Vec<ImportInput>,
    ) -> Result<Vec<ImportReport>, DomainError> {
        RocksdbStore::import_documents(self, inputs).await
    }

    async fn graph_store_write(
        &self,
        graph: &str,
        body: &str,
        format: SerializationFormat,
        mode: GraphWriteMode,
    ) -> Result<usize, DomainError> {
        RocksdbStore::graph_store_write(self, graph, body, format, mode).await
    }

    async fn graph_store_clear(&self, graph: &str) -> Result<usize, DomainError> {
        RocksdbStore::graph_store_clear(self, graph).await
    }

    async fn graph_store_read(&self, graph: &str) -> Result<Vec<Triple>, DomainError> {
        RocksdbStore::graph_store_read(self, graph).await
    }

    async fn triples_for_subject(&self, subject_iri: &str) -> Result<Vec<Triple>, DomainError> {
        self.triples.triples_for_subject(subject_iri).await
    }

    async fn ping(&self) -> Result<(), DomainError> {
        // Opening the database already proved it is reachable; a cheap read
        // confirms the handle still works.
        let db = self.db.clone();
        blocking(move || db.get_cf("meta", b"ping").map(|_| ())).await
    }
}
