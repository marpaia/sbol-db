//! The RocksDB-backed SBOL store: ingest plus the derived-view read surface,
//! the SPARQL read/write adapters, and the storage-trait implementations. Each
//! high-level write composes one atomic [`WriteBatch`]; async trait methods run
//! their RocksDB work on a blocking thread, while [`TripleSource`] (already
//! synchronous, driven inside the SPARQL evaluator's blocking task) calls the
//! engine directly.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use rocksdb::WriteBatch;
use sbol_db_core::{
    DomainError, GraphId, GraphRecord, ImportReport, IriString, NeighborhoodQuery,
    NeighborhoodResult, ObjectId, SbolObjectRecord, SerializationFormat, Triple,
};
use sbol_db_derive::{build_import_plan, compose_merged_input, to_rdf_format};
use sbol_db_rdf::{hash_bytes, rdf_graph_to_triples, GRAPH_IRI_PREFIX};
use sbol_db_storage::{
    build_accel_index, distinct_graph_iris, distinct_object_iris, AccelSolutions, AcceleratedQuery,
    AclStore, BatchSequenceMatch, CatalogSequenceRecord, ClassCount, CorpusCounts, CorpusStats,
    CorpusStatsStore, CursorPage, GraphFilter, GraphOverview, GraphStore, GraphTriplesPage,
    GraphWriteMode, IdGraphFilter, IdQuad, ImportInput, ImportOverwrite, LabStore,
    ListGraphsFilter, ListObjectsFilter, NamedGraphCatalogStore, NamedGraphQuery, NamedGraphRecord,
    NeighborhoodStore, ObjectStore, OntologyLoadReport, OntologyRecord, OntologyStore,
    OntologyTermRecord, PatternObject, PatternSubject, ResourceCatalogStore, ResourceOccurrence,
    ResourceQuery, ResourceRecord, SbolStore, SequenceCatalogStore, SequenceMatch, SequenceQuery,
    SequenceSearchOptions, SequenceSearchStore, TermId, TermKey, TermValue, TextSearchQuery,
    TextSearchStore, TripleChange, TriplePageQuery, TripleScanPage, TripleSource, TripleWriter,
    UpdateOutcome, SBH_CAN_VIEW, SBH_OWNED_BY,
};

use crate::codec::Term;
use crate::db::Db;
use crate::repo::accel::AccelRepository;
use crate::repo::catalog::{CatalogMutation, CatalogRepository};
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
    catalog: CatalogRepository,
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
            accel: AccelRepository::new(db.clone()),
            catalog: CatalogRepository::new(db.clone()),
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
            catalog: self.catalog.clone(),
        })
    }

    /// Stage the cascade delete of a document graph (registry, triples, and
    /// derived objects) into `batch`, mirroring [`GraphStore::delete_graph`].
    fn stage_delete_graph(
        &self,
        batch: &mut WriteBatch,
        mutation: &mut CatalogMutation,
        id: GraphId,
    ) -> Result<(), DomainError> {
        if self.graphs.stage_delete(batch, id)?.is_none() {
            return Ok(());
        }
        let iri = format!("{GRAPH_IRI_PREFIX}{}", id.0);
        let gid = Term::named(&iri).id();
        let deleted = self.triples.stage_delete_named_graph(batch, gid)?;
        self.catalog.stage_triple_delta(mutation, -(deleted as i64));
        self.objects.stage_delete_for_graph(batch, id)?;
        let delta = self.accel.stage_refresh(batch, &iri, &[])?;
        self.catalog.stage_projection_delta(
            mutation,
            &delta.old_resources,
            &delta.new_resources,
            &delta.old_sequences,
            &delta.new_sequences,
            &delta.old_types,
            &delta.new_types,
        )?;
        self.catalog.stage_delete_graph(batch, mutation, &iri)?;
        Ok(())
    }

    fn stage_import(
        &self,
        batch: &mut WriteBatch,
        mutation: &mut CatalogMutation,
        seen: &mut HashSet<Vec<u8>>,
        input: ImportInput,
    ) -> Result<ImportReport, DomainError> {
        // A merge folds the existing graph's triples into the incoming document
        // and becomes a replace of that graph.
        let input = if input.overwrite == ImportOverwrite::Merge {
            match &input.document_iri {
                Some(document_iri) => {
                    match self.graphs.id_by_document_iri(document_iri.as_str())? {
                        Some(old_id) => {
                            let old_graph_iri = format!("{GRAPH_IRI_PREFIX}{}", old_id.0);
                            let old_triples = self.triples.scan_pattern(
                                None,
                                None,
                                None,
                                Some(&GraphFilter::Iri(old_graph_iri)),
                                i64::MAX,
                            )?;
                            compose_merged_input(&old_triples, &input)?
                        }
                        None => ImportInput {
                            overwrite: ImportOverwrite::Replace,
                            ..input
                        },
                    }
                }
                None => input,
            }
        } else {
            input
        };

        // A replace drops the prior graph carrying this document IRI in the same
        // write batch, so the re-import neither collides with it nor leaves a
        // half-replaced state.
        if input.overwrite == ImportOverwrite::Replace {
            if let Some(document_iri) = &input.document_iri {
                if let Some(old_id) = self.graphs.id_by_document_iri(document_iri.as_str())? {
                    self.stage_delete_graph(batch, mutation, old_id)?;
                }
            }
        }

        let plan = build_import_plan(&input)?;
        let graph_record = self
            .graphs
            .stage_insert(batch, plan.graph_id, &plan.new_graph)?;
        let triple_count = self.triples.stage_insert(batch, seen, &plan.triples)?;
        self.catalog
            .stage_triple_delta(mutation, triple_count as i64);
        // The graph is freshly minted by the plan, so its post-write triple set
        // is exactly the document's triples.
        let delta = self
            .accel
            .stage_refresh(batch, plan.graph_iri.as_str(), &plan.triples)?;
        self.catalog.stage_projection_delta(
            mutation,
            &delta.old_resources,
            &delta.new_resources,
            &delta.old_sequences,
            &delta.new_sequences,
            &delta.old_types,
            &delta.new_types,
        )?;
        self.catalog.stage_ensure_graph(
            batch,
            mutation,
            NamedGraphRecord {
                id: graph_record.id,
                iri: plan.graph_iri.as_str().to_owned(),
                name: graph_record.name.clone(),
                description: graph_record.description.clone(),
                source_uri: graph_record.source_uri.clone(),
                serialization_format: Some(
                    graph_record.serialization_format.as_db_str().to_owned(),
                ),
                triple_count: Some(plan.triples.len() as u64),
                resource_count: Some(delta.new_resources.len() as u64),
                created_at: Some(graph_record.created_at),
                updated_at: Some(graph_record.updated_at),
            },
        )?;
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

/// Reconstruct the storage-level object summary from canonical RDF. Native
/// imports keep the eagerly materialized `objects` row; a verbatim corpus can
/// use this read-through path without first manufacturing native document
/// ownership records.
fn compatibility_object(
    triples: &TripleRepository,
    iri: &str,
) -> Result<Option<SbolObjectRecord>, DomainError> {
    let subject = PatternSubject::Iri(iri.to_owned());
    let rows = triples.scan_pattern(Some(&subject), None, None, None, i64::MAX)?;
    if rows.is_empty() {
        return Ok(None);
    }
    let index = build_accel_index(&rows);
    let Some(object) = index.objects.into_iter().find(|object| object.iri == iri) else {
        return Ok(None);
    };
    compatibility_object_from_meta(iri, object.meta).map(Some)
}

fn compatibility_object_from_meta(
    iri: &str,
    meta: sbol_db_storage::MetaRecord,
) -> Result<SbolObjectRecord, DomainError> {
    let encoded =
        serde_json::to_vec(&meta).map_err(|error| DomainError::Serialization(error.to_string()))?;
    let data = serde_json::to_value(&meta)
        .map_err(|error| DomainError::Serialization(error.to_string()))?;
    let first_literal =
        |values: &[sbol_db_storage::LitVal]| values.first().map(|value| value.value.clone());
    Ok(SbolObjectRecord {
        id: ObjectId(uuid::Uuid::from_bytes(Term::named(iri).id())),
        iri: IriString::unchecked(iri.to_owned()),
        sbol_class: meta
            .types
            .first()
            .cloned()
            .unwrap_or_else(|| "http://www.w3.org/2000/01/rdf-schema#Resource".to_owned()),
        display_id: first_literal(&meta.display_id),
        name: first_literal(&meta.name),
        description: first_literal(&meta.description),
        graph_id: None,
        types: meta.sbol_types,
        roles: meta.roles,
        data,
        content_hash: hash_bytes(&encoded),
    })
}

fn catalog_sequence_from_rdf(
    db: &Db,
    triples: &TripleRepository,
    iri: &str,
) -> Result<Option<CatalogSequenceRecord>, DomainError> {
    let Some(count) = db.get_cf("catalog_sequence_refcount", iri.as_bytes())? else {
        return Ok(None);
    };
    if count.len() != 8 {
        return Err(DomainError::Database(
            "invalid sequence catalog refcount".to_owned(),
        ));
    }
    let graph_count = u64::from_le_bytes(count.try_into().unwrap());
    let subject = PatternSubject::Iri(iri.to_owned());
    let rows = triples.scan_pattern(Some(&subject), None, None, None, i64::MAX)?;
    let mut rows: Vec<_> = rows
        .into_iter()
        .filter(|row| {
            matches!(
                row.predicate.as_str(),
                "http://sbols.org/v2#elements"
                    | "http://sbols.org/v3#elements"
                    | "http://sbols.org/v2#encoding"
                    | "http://sbols.org/v3#encoding"
            )
        })
        .collect();
    rows.sort_by(|left, right| left.graph_iri.cmp(&right.graph_iri));
    let mut elements = None;
    let mut encoding_iri = None;
    for row in rows {
        if row.predicate.as_str().ends_with("#elements") && elements.is_none() {
            if let sbol_db_core::ObjectTerm::Literal { value, .. } = row.object {
                elements = Some(value);
            }
        } else if row.predicate.as_str().ends_with("#encoding") && encoding_iri.is_none() {
            if let sbol_db_core::ObjectTerm::Iri(value) = row.object {
                encoding_iri = Some(value.into_inner());
            }
        }
    }
    Ok(Some(CatalogSequenceRecord {
        iri: iri.to_owned(),
        graph_count,
        alphabet: catalog_alphabet(encoding_iri.as_deref()),
        encoding_iri,
        elements,
    }))
}

fn catalog_alphabet(encoding: Option<&str>) -> Option<String> {
    let encoding = encoding?.to_ascii_lowercase();
    Some(
        if encoding.contains("protein") || encoding.contains("amino") {
            "PROTEIN"
        } else if encoding.contains("rna") {
            "RNA"
        } else if encoding.contains("dna")
            || encoding.contains("naseq")
            || encoding.contains("1207")
        {
            "DNA"
        } else {
            "OTHER"
        }
        .to_owned(),
    )
}

impl RocksdbStore {
    async fn import_document(&self, input: ImportInput) -> Result<ImportReport, DomainError> {
        let this = self.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            let mut mutation = this.catalog.begin_mutation()?;
            let mut seen = HashSet::new();
            let report = this.stage_import(&mut batch, &mut mutation, &mut seen, input)?;
            this.catalog.finish(&mut batch, mutation)?;
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
            let mut mutation = this.catalog.begin_mutation()?;
            let mut seen = HashSet::new();
            let mut reports = Vec::with_capacity(inputs.len());
            for input in inputs {
                reports.push(this.stage_import(&mut batch, &mut mutation, &mut seen, input)?);
            }
            this.catalog.finish(&mut batch, mutation)?;
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
            let mut mutation = this.catalog.begin_mutation()?;
            let mut seen = HashSet::new();
            let old_count = this
                .catalog
                .graph_by_iri(&graph)?
                .and_then(|record| record.triple_count)
                .unwrap_or_else(|| {
                    this.triples
                        .scan_pattern(
                            None,
                            None,
                            None,
                            Some(&GraphFilter::Iri(graph.clone())),
                            i64::MAX,
                        )
                        .map(|rows| rows.len() as u64)
                        .unwrap_or(0)
                });
            let inserted = if mode == GraphWriteMode::Replace {
                // Replace overwrites the graph atomically: delete only the triples
                // the new contents drop and insert the rest. A blanket clear plus
                // insert would lose any triple common to both, whose staged delete
                // the insert's already-present guard cannot see within one batch.
                this.triples
                    .stage_replace_graph(&mut batch, &mut seen, &graph, &triples)?
            } else {
                this.triples.stage_insert(&mut batch, &mut seen, &triples)?
            };
            // The graph's post-write triples: just the posted ones for Replace, or
            // the existing committed triples plus the posted ones for Merge.
            let post = if mode == GraphWriteMode::Replace {
                triples
            } else {
                let mut existing = this.triples.scan_pattern(
                    None,
                    None,
                    None,
                    Some(&GraphFilter::Iri(graph.clone())),
                    i64::MAX,
                )?;
                existing.extend(triples);
                existing
            };
            let delta = this.accel.stage_refresh(&mut batch, &graph, &post)?;
            this.catalog.stage_projection_delta(
                &mut mutation,
                &delta.old_resources,
                &delta.new_resources,
                &delta.old_sequences,
                &delta.new_sequences,
                &delta.old_types,
                &delta.new_types,
            )?;
            this.catalog
                .stage_triple_delta(&mut mutation, post.len() as i64 - old_count as i64);
            this.catalog.stage_graph_counts(
                &mut batch,
                &mut mutation,
                &graph,
                post.len() as u64,
                delta.new_resources.len() as u64,
            )?;
            this.catalog.finish(&mut batch, mutation)?;
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
            let mut mutation = this.catalog.begin_mutation()?;
            let deleted = this.triples.stage_clear_graph(&mut batch, Some(&graph))?;
            // The graph is now empty, so its accelerator indexes are dropped.
            let delta = this.accel.stage_refresh(&mut batch, &graph, &[])?;
            this.catalog.stage_projection_delta(
                &mut mutation,
                &delta.old_resources,
                &delta.new_resources,
                &delta.old_sequences,
                &delta.new_sequences,
                &delta.old_types,
                &delta.new_types,
            )?;
            this.catalog
                .stage_triple_delta(&mut mutation, -(deleted as i64));
            this.catalog
                .stage_graph_counts(&mut batch, &mut mutation, &graph, 0, 0)?;
            this.catalog.finish(&mut batch, mutation)?;
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

    async fn graph_store_read_page(
        &self,
        graph: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<TripleScanPage, DomainError> {
        let triples = self.triples.clone();
        let graph = graph.to_owned();
        let after = after.map(ToOwned::to_owned);
        blocking(move || triples.scan_graph_page(&graph, after.as_deref(), limit)).await
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
    catalog: CatalogRepository,
}

#[async_trait]
impl TripleWriter for RocksdbTripleWriter {
    async fn apply_update(&self, changes: Vec<TripleChange>) -> Result<UpdateOutcome, DomainError> {
        let triples = self.triples.clone();
        let db = self.db.clone();
        let accel = self.accel.clone();
        let catalog = self.catalog.clone();
        blocking(move || {
            let mut outcome = UpdateOutcome::default();
            let mut batch = WriteBatch::default();
            let mut mutation = catalog.begin_mutation()?;
            let mut seen = HashSet::new();
            // Per named graph, the inserts and deletes this update applies and
            // whether it was cleared, so each touched graph's accelerator indexes
            // can be rebuilt from its post-write triple set in the same batch.
            let mut inserts_by_graph: HashMap<String, Vec<Triple>> = HashMap::new();
            let mut deletes_by_graph: HashMap<String, Vec<Triple>> = HashMap::new();
            let mut cleared: HashSet<String> = HashSet::new();
            for change in &changes {
                match change {
                    TripleChange::Change { deletes, inserts } => {
                        outcome.deleted += triples.stage_delete(&mut batch, deletes)?;
                        outcome.inserted += triples.stage_insert(&mut batch, &mut seen, inserts)?;
                        for t in deletes {
                            if let Some(g) = &t.graph_iri {
                                deletes_by_graph
                                    .entry(g.as_str().to_owned())
                                    .or_default()
                                    .push(t.clone());
                            }
                        }
                        for t in inserts {
                            if let Some(g) = &t.graph_iri {
                                inserts_by_graph
                                    .entry(g.as_str().to_owned())
                                    .or_default()
                                    .push(t.clone());
                            }
                        }
                    }
                    TripleChange::Clear(graph) => {
                        outcome.deleted += triples
                            .stage_clear_graph(&mut batch, graph.as_ref().map(|i| i.as_str()))?;
                        if let Some(g) = graph {
                            cleared.insert(g.as_str().to_owned());
                        }
                    }
                }
            }
            let mut touched: HashSet<String> = cleared.clone();
            touched.extend(inserts_by_graph.keys().cloned());
            touched.extend(deletes_by_graph.keys().cloned());
            for graph in &touched {
                // Post-write triples: empty if the graph was cleared, otherwise the
                // committed triples minus this update's deletes; then plus its
                // inserts.
                let mut post: Vec<Triple> = if cleared.contains(graph) {
                    Vec::new()
                } else {
                    let dels = deletes_by_graph.get(graph);
                    triples
                        .scan_pattern(
                            None,
                            None,
                            None,
                            Some(&GraphFilter::Iri(graph.clone())),
                            i64::MAX,
                        )?
                        .into_iter()
                        .filter(|t| dels.is_none_or(|d| !d.contains(t)))
                        .collect()
                };
                if let Some(ins) = inserts_by_graph.get(graph) {
                    post.extend(ins.iter().cloned());
                }
                let delta = accel.stage_refresh(&mut batch, graph, &post)?;
                catalog.stage_projection_delta(
                    &mut mutation,
                    &delta.old_resources,
                    &delta.new_resources,
                    &delta.old_sequences,
                    &delta.new_sequences,
                    &delta.old_types,
                    &delta.new_types,
                )?;
                catalog.stage_graph_counts(
                    &mut batch,
                    &mut mutation,
                    graph,
                    post.len() as u64,
                    delta.new_resources.len() as u64,
                )?;
            }
            catalog.stage_triple_delta(
                &mut mutation,
                outcome.inserted as i64 - outcome.deleted as i64,
            );
            catalog.finish(&mut batch, mutation)?;
            db.write(batch)?;
            Ok(outcome)
        })
        .await
    }
}

#[async_trait]
impl ObjectStore for RocksdbStore {
    async fn get_object_by_iri(&self, iri: &str) -> Result<Option<SbolObjectRecord>, DomainError> {
        let accel = self.accel.clone();
        let catalog = self.catalog.clone();
        let iri = iri.to_owned();
        blocking(move || {
            catalog.ensure_ready()?;
            accel
                .resource(&iri)?
                .map(|resource| compatibility_object_from_meta(&resource.iri, resource.meta))
                .transpose()
        })
        .await
    }

    async fn get_objects_by_iris(
        &self,
        iris: &[&str],
    ) -> Result<Vec<SbolObjectRecord>, DomainError> {
        let accel = self.accel.clone();
        let catalog = self.catalog.clone();
        let owned: Vec<String> = iris.iter().map(|s| s.to_string()).collect();
        blocking(move || {
            catalog.ensure_ready()?;
            let mut out = Vec::with_capacity(owned.len());
            for iri in owned {
                if let Some(resource) = accel.resource(&iri)? {
                    out.push(compatibility_object_from_meta(
                        &resource.iri,
                        resource.meta,
                    )?);
                }
            }
            Ok(out)
        })
        .await
    }

    async fn list_objects(
        &self,
        filter: &ListObjectsFilter,
    ) -> Result<Vec<SbolObjectRecord>, DomainError> {
        let accel = self.accel.clone();
        let catalog = self.catalog.clone();
        let filter = filter.clone();
        blocking(move || {
            catalog.ensure_ready()?;
            let graph_iri = match filter.graph_id {
                Some(id) => catalog.graph(id)?.map(|graph| graph.iri),
                None => None,
            };
            let page = accel.resources(&ResourceQuery {
                after: filter.after_iri,
                limit: filter.limit,
                text: None,
                class: filter.sbol_class,
                role: filter.role,
                graph_iri,
            })?;
            page.items
                .into_iter()
                .map(|resource| compatibility_object_from_meta(&resource.iri, resource.meta))
                .collect()
        })
        .await
    }

    async fn get_object_iri_by_id(&self, id: ObjectId) -> Result<Option<String>, DomainError> {
        let objects = self.objects.clone();
        let triples = self.triples.clone();
        let db = self.db.clone();
        let catalog = self.catalog.clone();
        blocking(move || {
            catalog.ensure_ready()?;
            match objects.get_iri_by_id(id)? {
                Some(iri) => Ok(Some(iri)),
                None => {
                    let term_id = *id.0.as_bytes();
                    let Some(encoded) = db.get_cf("id2term", &term_id)? else {
                        return Ok(None);
                    };
                    let iri = match Term::decode(&encoded)? {
                        Term::Named(iri) => iri,
                        _ => return Ok(None),
                    };
                    Ok(compatibility_object(&triples, &iri)?.map(|record| record.iri.into_inner()))
                }
            }
        })
        .await
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
            if this.graphs.get(id)?.is_none() {
                return Ok(false);
            }
            let mut mutation = this.catalog.begin_mutation()?;
            this.stage_delete_graph(&mut batch, &mut mutation, id)?;
            this.catalog.finish(&mut batch, mutation)?;
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

    async fn graph_id_by_document_iri(
        &self,
        document_iri: &str,
    ) -> Result<Option<GraphId>, DomainError> {
        let graphs = self.graphs.clone();
        let document_iri = document_iri.to_owned();
        blocking(move || graphs.id_by_document_iri(&document_iri)).await
    }
}

#[async_trait]
impl TextSearchStore for RocksdbStore {
    async fn search_objects(
        &self,
        query: &TextSearchQuery,
    ) -> Result<(Vec<SbolObjectRecord>, i64), DomainError> {
        let objects = self.objects.clone();
        let query = query.clone();
        blocking(move || objects.search(&query)).await
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

    async fn align_candidates(&self, query: &str) -> Result<Vec<(String, String)>, DomainError> {
        let sequences = self.sequences.clone();
        let query = query.to_owned();
        blocking(move || sequences.align_candidates(&query)).await
    }

    async fn sequences_by_iris(
        &self,
        iris: &[String],
    ) -> Result<Vec<(String, String)>, DomainError> {
        let sequences = self.sequences.clone();
        let iris = iris.to_vec();
        blocking(move || sequences.sequences_by_iris(&iris)).await
    }

    async fn all_nucleotide_sequences(&self) -> Result<Vec<(String, String)>, DomainError> {
        let sequences = self.sequences.clone();
        blocking(move || sequences.all_nucleotide_sequences()).await
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
impl ResourceCatalogStore for RocksdbStore {
    async fn catalog_resource(&self, iri: &str) -> Result<Option<ResourceRecord>, DomainError> {
        let accel = self.accel.clone();
        let catalog = self.catalog.clone();
        let iri = iri.to_owned();
        blocking(move || {
            catalog.ensure_ready()?;
            accel.resource(&iri)
        })
        .await
    }

    async fn catalog_resource_occurrences(
        &self,
        iri: &str,
    ) -> Result<Vec<ResourceOccurrence>, DomainError> {
        let accel = self.accel.clone();
        let catalog = self.catalog.clone();
        let iri = iri.to_owned();
        blocking(move || {
            catalog.ensure_ready()?;
            accel.resource_occurrences(&iri)
        })
        .await
    }

    async fn catalog_resources(
        &self,
        query: &ResourceQuery,
    ) -> Result<CursorPage<ResourceRecord>, DomainError> {
        let accel = self.accel.clone();
        let catalog = self.catalog.clone();
        let query = query.clone();
        blocking(move || {
            catalog.ensure_ready()?;
            accel.resources(&query)
        })
        .await
    }

    async fn catalog_resources_by_iris(
        &self,
        iris: &[String],
    ) -> Result<Vec<ResourceRecord>, DomainError> {
        let accel = self.accel.clone();
        let catalog = self.catalog.clone();
        let iris = iris.to_vec();
        blocking(move || {
            catalog.ensure_ready()?;
            iris.into_iter()
                .filter_map(|iri| match accel.resource(&iri) {
                    Ok(Some(resource)) => Some(Ok(resource)),
                    Ok(None) => None,
                    Err(error) => Some(Err(error)),
                })
                .collect()
        })
        .await
    }

    async fn catalog_top_classes(&self, limit: u32) -> Result<Vec<ClassCount>, DomainError> {
        let catalog = self.catalog.clone();
        blocking(move || catalog.top_classes(i64::from(limit.clamp(1, 500)))).await
    }
}

#[async_trait]
impl SequenceCatalogStore for RocksdbStore {
    async fn catalog_sequence(
        &self,
        iri: &str,
    ) -> Result<Option<CatalogSequenceRecord>, DomainError> {
        let db = self.db.clone();
        let triples = self.triples.clone();
        let catalog = self.catalog.clone();
        let iri = iri.to_owned();
        blocking(move || {
            catalog.ensure_ready()?;
            catalog_sequence_from_rdf(&db, &triples, &iri)
        })
        .await
    }

    async fn catalog_sequences(
        &self,
        query: &SequenceQuery,
    ) -> Result<CursorPage<CatalogSequenceRecord>, DomainError> {
        let db = self.db.clone();
        let triples = self.triples.clone();
        let accel = self.accel.clone();
        let catalog = self.catalog.clone();
        let query = query.clone();
        blocking(move || {
            catalog.ensure_ready()?;
            let limit = query.limit.clamp(1, 500) as usize;
            let after = query.after.as_deref();
            let needle = query.text.as_ref().map(|value| value.to_lowercase());
            let mut iris = Vec::with_capacity(limit + 1);
            db.for_each_prefix_after(
                "catalog_sequence_refcount",
                b"",
                after.map(str::as_bytes),
                |key, _| {
                    let iri = std::str::from_utf8(key).map_err(|_| {
                        DomainError::Database("non-utf8 sequence catalog IRI".into())
                    })?;
                    if after.is_some_and(|cursor| iri <= cursor) {
                        return Ok(true);
                    }
                    let matches = needle.as_ref().is_none_or(|needle| {
                        iri.to_lowercase().contains(needle)
                            || accel.resource(iri).is_ok_and(|resource| {
                                resource.is_some_and(|resource| {
                                    serde_json::to_string(&resource.meta)
                                        .is_ok_and(|meta| meta.to_lowercase().contains(needle))
                                })
                            })
                    });
                    if matches {
                        iris.push(iri.to_owned());
                    }
                    Ok(iris.len() < limit + 1)
                },
            )?;
            let has_more = iris.len() > limit;
            iris.truncate(limit);
            let next_cursor = has_more.then(|| iris.last().cloned()).flatten();
            let mut items = Vec::with_capacity(iris.len());
            for iri in iris {
                if let Some(sequence) = catalog_sequence_from_rdf(&db, &triples, &iri)? {
                    items.push(sequence);
                }
            }
            Ok(CursorPage { items, next_cursor })
        })
        .await
    }
}

#[async_trait]
impl CorpusStatsStore for RocksdbStore {
    async fn catalog_stats(&self) -> Result<CorpusStats, DomainError> {
        let catalog = self.catalog.clone();
        blocking(move || catalog.stats()).await
    }
}

#[async_trait]
impl NamedGraphCatalogStore for RocksdbStore {
    async fn catalog_graph(&self, id: GraphId) -> Result<Option<NamedGraphRecord>, DomainError> {
        let catalog = self.catalog.clone();
        blocking(move || catalog.graph(id)).await
    }

    async fn catalog_graphs(
        &self,
        query: &NamedGraphQuery,
    ) -> Result<CursorPage<NamedGraphRecord>, DomainError> {
        let catalog = self.catalog.clone();
        let query = query.clone();
        blocking(move || catalog.graphs(&query)).await
    }

    async fn catalog_graph_triples(
        &self,
        id: GraphId,
        query: &TriplePageQuery,
    ) -> Result<Option<CursorPage<Triple>>, DomainError> {
        let catalog = self.catalog.clone();
        let Some(graph) = blocking(move || catalog.graph(id)).await? else {
            return Ok(None);
        };
        let page = RocksdbStore::graph_store_read_page(
            self,
            &graph.iri,
            query.after.as_deref(),
            query.limit.clamp(1, 5_000) as usize,
        )
        .await?;
        Ok(Some(CursorPage {
            items: page.items,
            next_cursor: page.next_cursor,
        }))
    }
}

#[async_trait]
impl AclStore for RocksdbStore {
    async fn owned_graphs(&self, owner_iri: &str) -> Result<Vec<String>, DomainError> {
        let triples = self.triples.clone();
        let object = PatternObject::Iri(owner_iri.to_owned());
        let scanned = blocking(move || {
            triples.scan_pattern(None, Some(SBH_OWNED_BY), Some(&object), None, i64::MAX)
        })
        .await?;
        Ok(distinct_graph_iris(scanned))
    }

    async fn viewable_objects(&self, owner_iri: &str) -> Result<Vec<String>, DomainError> {
        let triples = self.triples.clone();
        let subject = PatternSubject::Iri(owner_iri.to_owned());
        let scanned = blocking(move || {
            triples.scan_pattern(Some(&subject), Some(SBH_CAN_VIEW), None, None, i64::MAX)
        })
        .await?;
        Ok(distinct_object_iris(scanned))
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

    async fn graph_store_read_page(
        &self,
        graph: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<TripleScanPage, DomainError> {
        RocksdbStore::graph_store_read_page(self, graph, after, limit).await
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

#[cfg(test)]
mod tests {
    use super::*;
    use sbol_db_core::{ObjectTerm, SubjectTerm};
    use sbol_db_storage::Migrator;

    const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

    fn iri_triple(graph: &str, subject: &str, predicate: &str, object: &str) -> Triple {
        Triple {
            graph_iri: Some(IriString::unchecked(graph.to_owned())),
            subject: SubjectTerm::Iri(IriString::unchecked(subject.to_owned())),
            predicate: IriString::unchecked(predicate.to_owned()),
            object: ObjectTerm::Iri(IriString::unchecked(object.to_owned())),
        }
    }

    fn literal_triple(graph: &str, subject: &str, predicate: &str, value: &str) -> Triple {
        Triple {
            graph_iri: Some(IriString::unchecked(graph.to_owned())),
            subject: SubjectTerm::Iri(IriString::unchecked(subject.to_owned())),
            predicate: IriString::unchecked(predicate.to_owned()),
            object: ObjectTerm::Literal {
                value: value.to_owned(),
                datatype: IriString::unchecked(XSD_STRING.to_owned()),
                language: None,
            },
        }
    }

    #[tokio::test]
    async fn migration_builds_the_catalog_before_admin_reads() {
        let directory = tempfile::tempdir().unwrap();
        let db = Db::open(directory.path()).unwrap();
        let graph = "https://example.org/public";
        let second_graph = "https://example.org/user/alice";
        let object = "https://example.org/part";
        let sequence = "https://example.org/sequence";
        let other = "https://example.org/z-other";
        let graph_rows = vec![
            iri_triple(
                graph,
                object,
                RDF_TYPE,
                "http://sbols.org/v2#ComponentDefinition",
            ),
            literal_triple(graph, object, "http://sbols.org/v2#displayId", "part"),
            iri_triple(graph, sequence, RDF_TYPE, "http://sbols.org/v2#Sequence"),
            literal_triple(graph, sequence, "http://sbols.org/v2#elements", "AACCGGTT"),
        ];
        let second_graph_rows = vec![
            iri_triple(
                second_graph,
                object,
                RDF_TYPE,
                "http://sbols.org/v2#ComponentDefinition",
            ),
            iri_triple(
                second_graph,
                other,
                RDF_TYPE,
                "http://sbols.org/v2#Collection",
            ),
        ];
        let rows: Vec<_> = graph_rows
            .iter()
            .chain(&second_graph_rows)
            .cloned()
            .collect();
        let triples = TripleRepository::new(db.clone());
        let mut batch = WriteBatch::default();
        triples
            .stage_bulk_insert(&mut batch, &rows)
            .expect("stage canonical RDF");
        AccelRepository::new(db.clone())
            .stage_refresh(&mut batch, graph, &graph_rows)
            .expect("stage RDF accelerator metadata");
        AccelRepository::new(db.clone())
            .stage_refresh(&mut batch, second_graph, &second_graph_rows)
            .expect("stage second graph accelerator metadata");
        batch.put_cf(&db.cf("seq_sketch"), sequence.as_bytes(), []);
        for iri in [object, sequence, other] {
            batch.put_cf(
                &db.cf("object_pagerank"),
                iri.as_bytes(),
                1_f64.to_le_bytes(),
            );
        }
        db.write(batch).unwrap();

        let store = RocksdbStore::new(db.clone());
        assert!(matches!(
            store.catalog_stats().await,
            Err(DomainError::Unavailable(_))
        ));
        assert!(matches!(
            store
                .catalog_resources(&ResourceQuery {
                    limit: 1,
                    ..ResourceQuery::default()
                })
                .await,
            Err(DomainError::Unavailable(_))
        ));
        assert!(matches!(
            store
                .catalog_graphs(&NamedGraphQuery {
                    limit: 1,
                    ..NamedGraphQuery::default()
                })
                .await,
            Err(DomainError::Unavailable(_))
        ));

        let migrator = crate::RocksdbMigrator::new(db);
        migrator.run_migrations().await.unwrap();
        migrator.run_migrations().await.unwrap();

        let stats = store.catalog_stats().await.unwrap();
        assert_eq!(stats.resources, 3);
        assert_eq!(stats.named_graphs, 2);
        assert_eq!(stats.triples, rows.len() as u64);
        assert_eq!(stats.sequences, 1);

        let graphs = store
            .catalog_graphs(&NamedGraphQuery {
                limit: 10,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(graphs.items.len(), 2);

        let part = store.catalog_resource(object).await.unwrap().unwrap();
        assert_eq!(part.graph_count, 2);
        assert_eq!(part.meta.display_id[0].value, "part");
        assert!(store.catalog_resource(other).await.unwrap().is_some());

        let matches = store
            .search("AACCGGTT", SequenceSearchOptions::default())
            .await
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].sequence_iri, sequence);
    }
}
