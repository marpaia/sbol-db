use sbol_db_core::{DomainError, ImportReport, IriString, SerializationFormat, Triple};
use sbol_db_derive::{build_import_plan, to_rdf_format};
use sbol_db_rdf::rdf_graph_to_triples;

use crate::repo::{
    AccelRepository, GraphRepository, LabRepository, NeighborhoodRepository, OntologyRepository,
    ProjectionEvent, ProjectionEventRepository, SbolObjectRepository, SequenceSearchRepository,
    TripleRepository, TypedProjectionRepository, ValidationRepository,
};
use crate::PgPool;

use sbol_db_storage::{GraphWriteMode, ImportInput};

pub struct SbolObjectService {
    pool: PgPool,
    graphs: GraphRepository,
    objects: SbolObjectRepository,
    triples: TripleRepository,
    accel: AccelRepository,
    validation: ValidationRepository,
    projection: ProjectionEventRepository,
    typed: TypedProjectionRepository,
    neighborhood: NeighborhoodRepository,
    sequence_search: SequenceSearchRepository,
    ontology: OntologyRepository,
    lab: LabRepository,
}

/// Per-call cap on rows returned by a Graph Store `GET`. Far above any real
/// single-graph payload SynBioHub would round-trip; a guard against a
/// pathological whole-graph serialize.
const GRAPH_READ_LIMIT: i64 = 5_000_000;

impl SbolObjectService {
    pub fn new(pool: PgPool) -> Self {
        Self {
            graphs: GraphRepository::new(pool.clone()),
            objects: SbolObjectRepository::new(pool.clone()),
            triples: TripleRepository::new(pool.clone()),
            accel: AccelRepository::new(pool.clone(), TripleRepository::new(pool.clone())),
            validation: ValidationRepository::new(pool.clone()),
            projection: ProjectionEventRepository::new(pool.clone()),
            typed: TypedProjectionRepository::new(pool.clone()),
            neighborhood: NeighborhoodRepository::new(pool.clone()),
            sequence_search: SequenceSearchRepository::new(pool.clone()),
            ontology: OntologyRepository::new(pool.clone()),
            lab: LabRepository::new(pool.clone()),
            pool,
        }
    }

    pub fn lab(&self) -> &LabRepository {
        &self.lab
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Cheap liveness probe against the connection pool. Used by the
    /// HTTP `/readyz` endpoint; intentionally trivial so it can't itself
    /// stall on application state.
    pub async fn ping(&self) -> Result<(), DomainError> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(|e| DomainError::Database(e.to_string()))
    }

    pub fn graphs(&self) -> &GraphRepository {
        &self.graphs
    }

    pub fn objects(&self) -> &SbolObjectRepository {
        &self.objects
    }

    pub fn triples(&self) -> &TripleRepository {
        &self.triples
    }

    pub fn accel(&self) -> &AccelRepository {
        &self.accel
    }

    pub fn neighborhood(&self) -> &NeighborhoodRepository {
        &self.neighborhood
    }

    pub fn sequence_search(&self) -> &SequenceSearchRepository {
        &self.sequence_search
    }

    pub fn ontology(&self) -> &OntologyRepository {
        &self.ontology
    }

    /// Atomically import a batch of documents inside one Postgres transaction.
    /// Either every document commits or none do — there is no half-imported
    /// state. The implementation is sequential per-document inside the shared
    /// transaction; the caller controls batch composition. Per-document
    /// validation runs and projection events are still recorded individually
    /// (so the batch shows up as N separate document_imported events), but
    /// they share the outer transaction's atomicity.
    ///
    /// Callers wanting partial-success semantics for corpus-scale onboarding
    /// should fan out to [`import_document`] themselves; the CLI directory
    /// import is the reference for that pattern.
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

    pub async fn import_document(&self, input: ImportInput) -> Result<ImportReport, DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let report = self.import_into_conn(&mut tx, input).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(report)
    }

    /// Graph Store HTTP Protocol write: one ingest mode feeding the shared,
    /// graph-owned store. Parses `body` as RDF and writes its triples
    /// **verbatim** into `graph` (registered via [`TripleRepository::ensure_graph`]),
    /// with no SBOL interpretation at write time. `Merge` appends; `Replace`
    /// first clears the graph; both run in one transaction. Returns the inserted
    /// triple count.
    ///
    /// This is the same storage substrate [`Self::import_document`] writes to —
    /// the difference is only how the triples arrive (a posted RDF graph here, a
    /// parsed/validated SBOL document there). The SBOL3 typed view
    /// ([`Self::apply_sbol_view`]) is a derivation over a graph's triples; for
    /// graphs populated this way it is produced by the asynchronous
    /// reprojection path rather than inline (see the derived-view work).
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

    /// Graph Store CRUD `DELETE`: drop a named graph entirely (its triples and
    /// `sbol_graphs` registry row). Returns the number of triples removed.
    pub async fn graph_store_clear(&self, graph: &str) -> Result<usize, DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let deleted = self.triples.clear_graph(&mut tx, Some(graph)).await?;
        self.triples.delete_graph(&mut tx, graph).await?;
        AccelRepository::mark_dirty(&mut tx, graph).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(deleted)
    }

    /// Graph Store CRUD `GET`: read back a named graph's triples.
    pub async fn graph_store_read(&self, graph: &str) -> Result<Vec<Triple>, DomainError> {
        self.triples
            .triples_for_graph(Some(graph), GRAPH_READ_LIMIT)
            .await
    }

    async fn import_into_conn(
        &self,
        conn: &mut sqlx::PgConnection,
        input: ImportInput,
    ) -> Result<ImportReport, DomainError> {
        let plan = build_import_plan(&input)?;

        // A document is an `sbol3`-kind graph that owns its triples. Register
        // the graph row (under the id minted in the plan), then write the
        // document's triples and derived SBOL view into it.
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
        let typed_counts = self.typed.upsert_all(&mut *conn, &plan.projections).await?;
        let recorded = self
            .validation
            .record_run(
                &mut *conn,
                &plan.target_iri,
                Some(plan.graph_id),
                "sbol-rs",
                Some(sbol::SPEC_VERSION),
                "sbol3-3.1.0",
                &plan.validation,
            )
            .await?;

        self.projection
            .append(
                &mut *conn,
                ProjectionEvent {
                    event_type: "document_imported".to_owned(),
                    subject_iri: Some(IriString::unchecked(plan.target_iri.clone())),
                    graph_iri: Some(plan.graph_iri.clone()),
                    payload: serde_json::json!({
                        "graph_id": plan.graph_id.as_uuid(),
                        "object_count": object_count,
                        "triple_count": triple_count,
                        "typed_counts": typed_counts,
                    }),
                },
            )
            .await?;

        Ok(ImportReport {
            graph_id: plan.graph_id,
            object_count,
            triple_count,
            validation_status: recorded.status,
            validation_issue_count: recorded.issue_count,
        })
    }
}

fn db_err<E: std::fmt::Display>(e: E) -> DomainError {
    DomainError::Database(e.to_string())
}
