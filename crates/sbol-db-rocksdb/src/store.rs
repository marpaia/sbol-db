//! The Oxigraph-backed SBOL store, exposed under the historical `RocksdbStore`
//! name. Triples and native SPARQL live in Oxigraph; the derived projections
//! (objects, graphs, ontology, sequences), the job queue, the lab dashboard,
//! and the SynBioHub accelerator index live in the SQLite companion.
//!
//! There is no cross-engine transaction: an import writes its triples to
//! Oxigraph first, then its projections to the companion in one SQLite
//! transaction (the companion graph row is the commit witness; re-inserting
//! identical quads into Oxigraph is idempotent).

use std::sync::Arc;

use async_trait::async_trait;
use oxigraph::store::Store;
use oxrdf::{GraphNameRef, NamedNode};
use sbol_db_core::{
    DomainError, GraphId, GraphRecord, ImportReport, IriString, NeighborhoodQuery,
    NeighborhoodResult, ObjectId, SbolObjectRecord, SerializationFormat, Triple,
};
use sbol_db_derive::{build_import_plan, to_rdf_format};
use sbol_db_rdf::{rdf_graph_to_triples, GRAPH_IRI_PREFIX};
use sbol_db_sparql::NativeSparql;
use sbol_db_sqlite::repo::{
    AccelRepository as SqliteAccelMark, GraphRepository, LabRepository, OntologyRepository,
    SbolObjectRepository, SequenceSearchRepository,
};
use sbol_db_sqlite::SqlitePool;
use sbol_db_storage::{
    BatchSequenceMatch, ClassCount, CorpusCounts, GraphOverview, GraphStore, GraphTriplesPage,
    GraphWriteMode, ImportInput, LabStore, ListGraphsFilter, ListObjectsFilter, NeighborhoodStore,
    ObjectStore, OntologyLoadReport, OntologyRecord, OntologyStore, OntologyTermRecord, SbolStore,
    SequenceMatch, SequenceSearchOptions, SequenceSearchStore, TripleSource, TripleWriter,
};

use crate::accel::AccelRepository;
use crate::convert::triple_to_quad;
use crate::db::OxigraphDb;
use crate::db_err;
use crate::neighborhood;
use crate::sparql::OxigraphNativeSparql;
use crate::triple_source::OxigraphTripleSource;
use crate::triple_writer::OxigraphTripleWriter;

/// Per-call cap on a Graph Store `GET`, matching the other backends.
const GRAPH_READ_LIMIT: i64 = 5_000_000;

/// The Oxigraph-backed SBOL store. Cloneable; all clones share the one Oxigraph
/// handle and the one SQLite companion pool.
#[derive(Clone)]
pub struct RocksdbStore {
    store: Store,
    pool: SqlitePool,
    graphs: GraphRepository,
    objects: SbolObjectRepository,
    ontology: OntologyRepository,
    sequences: SequenceSearchRepository,
    lab: LabRepository,
    accel: AccelRepository,
}

impl RocksdbStore {
    pub fn new(db: OxigraphDb) -> Self {
        let OxigraphDb { store, pool } = db;
        Self {
            graphs: GraphRepository::new(pool.clone()),
            objects: SbolObjectRepository::new(pool.clone()),
            ontology: OntologyRepository::new(pool.clone()),
            sequences: SequenceSearchRepository::new(pool.clone()),
            lab: LabRepository::new(pool.clone()),
            accel: AccelRepository::new(pool.clone(), store.clone()),
            store,
            pool,
        }
    }

    fn triple_source_inner(&self) -> OxigraphTripleSource {
        OxigraphTripleSource {
            store: self.store.clone(),
            accel: self.accel.clone(),
        }
    }

    pub fn triple_source(&self) -> Arc<dyn TripleSource> {
        Arc::new(self.triple_source_inner())
    }

    pub fn triple_writer(&self) -> Arc<dyn TripleWriter> {
        Arc::new(OxigraphTripleWriter {
            store: self.store.clone(),
            accel: self.accel.clone(),
        })
    }

    pub fn native_sparql(&self) -> Arc<dyn NativeSparql> {
        Arc::new(OxigraphNativeSparql::new(self.store.clone()))
    }

    /// Insert `triples` into Oxigraph with set semantics, returning the number of
    /// quads that were not already present. One transaction commits them all.
    fn insert_triples_counting(&self, triples: &[Triple]) -> Result<usize, DomainError> {
        let mut txn = self
            .store
            .start_transaction()
            .map_err(|e| DomainError::Database(format!("oxigraph txn: {e}")))?;
        let mut inserted = 0usize;
        for triple in triples {
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
                inserted += 1;
            }
        }
        txn.commit()
            .map_err(|e| DomainError::Database(format!("oxigraph commit: {e}")))?;
        Ok(inserted)
    }

    /// Clear every quad in a named graph, returning how many were removed.
    fn clear_graph_counting(&self, graph: &str) -> Result<usize, DomainError> {
        let node = NamedNode::new_unchecked(graph);
        let quads: Vec<_> = self
            .store
            .quads_for_pattern(
                None,
                None,
                None,
                Some(GraphNameRef::NamedNode(node.as_ref())),
            )
            .collect::<Result<_, _>>()
            .map_err(|e| DomainError::Database(format!("oxigraph clear scan: {e}")))?;
        let count = quads.len();
        if count == 0 {
            return Ok(0);
        }
        let mut txn = self
            .store
            .start_transaction()
            .map_err(|e| DomainError::Database(format!("oxigraph txn: {e}")))?;
        for quad in &quads {
            txn.remove(quad.as_ref());
        }
        txn.commit()
            .map_err(|e| DomainError::Database(format!("oxigraph commit: {e}")))?;
        Ok(count)
    }

    fn read_graph_triples(&self, graph: &str, limit: i64) -> Result<Vec<Triple>, DomainError> {
        self.triple_source_inner()
            .triples_for_graph(Some(graph), limit)
    }

    async fn import_one(&self, input: ImportInput) -> Result<ImportReport, DomainError> {
        let plan = build_import_plan(&input)?;

        // Triples first (Oxigraph); then projections (companion). The companion
        // graph row is the commit witness.
        let triple_count = {
            let this = self.clone();
            let triples = plan.triples.clone();
            tokio::task::spawn_blocking(move || this.insert_triples_counting(&triples))
                .await
                .map_err(|e| DomainError::Database(format!("oxigraph import task: {e}")))??
        };

        let mut tx = self.pool.begin().await.map_err(db_err)?;
        self.graphs
            .insert(&mut tx, plan.graph_id, plan.new_graph)
            .await?;
        for summary in &plan.summaries {
            self.objects
                .upsert(&mut tx, summary, Some(plan.graph_id))
                .await?;
        }
        for sequence in &plan.projections.sequences {
            self.sequences.upsert_sequence(&mut tx, sequence).await?;
        }
        SqliteAccelMark::mark_dirty(&mut tx, plan.graph_iri.as_str()).await?;
        tx.commit().await.map_err(db_err)?;

        Ok(ImportReport {
            graph_id: plan.graph_id,
            object_count: plan.summaries.len(),
            triple_count,
            validation_status: plan.validation_status,
            validation_issue_count: plan.validation_issue_count,
        })
    }
}

#[async_trait]
impl ObjectStore for RocksdbStore {
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
impl GraphStore for RocksdbStore {
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
        let graph_iri = format!("{GRAPH_IRI_PREFIX}{}", id.0);
        let this = self.clone();
        let iri = graph_iri.clone();
        tokio::task::spawn_blocking(move || this.clear_graph_counting(&iri))
            .await
            .map_err(|e| DomainError::Database(format!("oxigraph delete task: {e}")))??;
        let removed = self.graphs.delete(id).await?;
        self.accel.mark_dirty_pool(&graph_iri).await?;
        Ok(removed)
    }

    async fn graph_exists_by_hash(&self, hash: &[u8]) -> Result<bool, DomainError> {
        self.graphs.exists_by_hash(hash).await
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
impl NeighborhoodStore for RocksdbStore {
    async fn walk(&self, query: &NeighborhoodQuery) -> Result<NeighborhoodResult, DomainError> {
        let source = self.triple_source_inner();
        neighborhood::walk(&source, &self.objects, query).await
    }
}

#[async_trait]
impl SequenceSearchStore for RocksdbStore {
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
impl LabStore for RocksdbStore {
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
impl SbolStore for RocksdbStore {
    async fn import_document(&self, input: ImportInput) -> Result<ImportReport, DomainError> {
        self.import_one(input).await
    }

    async fn import_documents(
        &self,
        inputs: Vec<ImportInput>,
    ) -> Result<Vec<ImportReport>, DomainError> {
        let mut reports = Vec::with_capacity(inputs.len());
        for input in inputs {
            reports.push(self.import_one(input).await?);
        }
        Ok(reports)
    }

    async fn graph_store_write(
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

        let this = self.clone();
        let graph_owned = graph.to_owned();
        let inserted = tokio::task::spawn_blocking(move || {
            if mode == GraphWriteMode::Replace {
                this.clear_graph_counting(&graph_owned)?;
            }
            this.insert_triples_counting(&triples)
        })
        .await
        .map_err(|e| DomainError::Database(format!("oxigraph write task: {e}")))??;

        self.accel.mark_dirty_pool(graph).await?;
        Ok(inserted)
    }

    async fn graph_store_clear(&self, graph: &str) -> Result<usize, DomainError> {
        let this = self.clone();
        let graph_owned = graph.to_owned();
        let deleted = tokio::task::spawn_blocking(move || this.clear_graph_counting(&graph_owned))
            .await
            .map_err(|e| DomainError::Database(format!("oxigraph clear task: {e}")))??;
        self.accel.mark_dirty_pool(graph).await?;
        Ok(deleted)
    }

    async fn graph_store_read(&self, graph: &str) -> Result<Vec<Triple>, DomainError> {
        let this = self.clone();
        let graph_owned = graph.to_owned();
        tokio::task::spawn_blocking(move || this.read_graph_triples(&graph_owned, GRAPH_READ_LIMIT))
            .await
            .map_err(|e| DomainError::Database(format!("oxigraph read task: {e}")))?
    }

    async fn triples_for_subject(&self, subject_iri: &str) -> Result<Vec<Triple>, DomainError> {
        let this = self.clone();
        let subject = subject_iri.to_owned();
        tokio::task::spawn_blocking(move || {
            this.triple_source_inner().triples_for_subject(&subject)
        })
        .await
        .map_err(|e| DomainError::Database(format!("oxigraph subject task: {e}")))?
    }

    async fn ping(&self) -> Result<(), DomainError> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(db_err)
    }
}
