use sbol::{Document, RdfFormat};
use sbol_db_core::{DomainError, ImportReport, IriString, NewDocument, SerializationFormat};
use sbol_db_rdf::{
    document_to_projections, document_to_quads, document_to_summaries, hash_bytes, GRAPH_IRI_PREFIX,
};

use crate::repo::{
    DocumentRepository, NeighborhoodRepository, OntologyRepository, ProjectionEvent,
    ProjectionEventRepository, QuadRepository, SbolObjectRepository, SequenceSearchRepository,
    TypedProjectionRepository, ValidationRepository,
};
use crate::PgPool;

pub struct SbolObjectService {
    pool: PgPool,
    documents: DocumentRepository,
    objects: SbolObjectRepository,
    quads: QuadRepository,
    validation: ValidationRepository,
    projection: ProjectionEventRepository,
    typed: TypedProjectionRepository,
    neighborhood: NeighborhoodRepository,
    sequence_search: SequenceSearchRepository,
    ontology: OntologyRepository,
}

pub struct ImportInput {
    pub body: String,
    pub format: SerializationFormat,
    pub source_uri: Option<String>,
    pub document_iri: Option<IriString>,
    pub created_by: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
}

impl SbolObjectService {
    pub fn new(pool: PgPool) -> Self {
        Self {
            documents: DocumentRepository::new(pool.clone()),
            objects: SbolObjectRepository::new(pool.clone()),
            quads: QuadRepository::new(pool.clone()),
            validation: ValidationRepository::new(pool.clone()),
            projection: ProjectionEventRepository::new(pool.clone()),
            typed: TypedProjectionRepository::new(pool.clone()),
            neighborhood: NeighborhoodRepository::new(pool.clone()),
            sequence_search: SequenceSearchRepository::new(pool.clone()),
            ontology: OntologyRepository::new(pool.clone()),
            pool,
        }
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

    pub fn documents(&self) -> &DocumentRepository {
        &self.documents
    }

    pub fn objects(&self) -> &SbolObjectRepository {
        &self.objects
    }

    pub fn quads(&self) -> &QuadRepository {
        &self.quads
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

    pub async fn import_document(&self, input: ImportInput) -> Result<ImportReport, DomainError> {
        let rdf_format = to_rdf_format(input.format)?;
        let doc = Document::read(&input.body, rdf_format)
            .map_err(|e| DomainError::Parse(e.to_string()))?;

        let report = doc.validate();

        let body_hash = hash_bytes(input.body.as_bytes());

        let target_iri = input
            .document_iri
            .clone()
            .map(|i| i.into_inner())
            .unwrap_or_else(|| {
                doc.namespaces()
                    .next()
                    .map(|i| i.as_str().to_owned())
                    .unwrap_or_else(|| format!("urn:sbol-db:import:{}", uuid::Uuid::new_v4()))
            });

        let mut tx = self.pool.begin().await.map_err(db_err)?;

        let raw_payload = serde_json::to_value(triples_json_snapshot(&doc))?;

        let document_id = self
            .documents
            .insert(
                &mut tx,
                NewDocument {
                    document_iri: input.document_iri.clone(),
                    name: input.name,
                    description: input.description,
                    serialization_format: input.format,
                    source_uri: input.source_uri,
                    raw_payload: Some(raw_payload),
                    content_hash: body_hash,
                    created_by: input.created_by,
                },
            )
            .await?;

        let summaries = document_to_summaries(&doc);
        let object_count = summaries.len();

        for slice in &summaries {
            self.objects
                .upsert(&mut tx, &slice.summary, Some(document_id))
                .await?;
        }

        let typed_projections = document_to_projections(&doc);
        let typed_counts = self.typed.upsert_all(&mut tx, &typed_projections).await?;

        let graph_iri =
            IriString::unchecked(format!("{}{}", GRAPH_IRI_PREFIX, document_id.as_uuid()));
        let quads = document_to_quads(&doc, &graph_iri);
        let quad_count = self
            .quads
            .replace_document_graph(&mut tx, document_id, &quads)
            .await?;

        let recorded = self
            .validation
            .record_run(
                &mut tx,
                &target_iri,
                Some(document_id),
                "sbol-rs",
                Some(sbol::SPEC_VERSION),
                "sbol3-3.1.0",
                &report,
            )
            .await?;

        self.projection
            .append(
                &mut tx,
                ProjectionEvent {
                    event_type: "document_imported".to_owned(),
                    subject_iri: Some(IriString::unchecked(target_iri.clone())),
                    graph_iri: Some(graph_iri.clone()),
                    payload: serde_json::json!({
                        "document_id": document_id.as_uuid(),
                        "object_count": object_count,
                        "quad_count": quad_count,
                        "typed_counts": typed_counts,
                    }),
                },
            )
            .await?;

        tx.commit().await.map_err(db_err)?;

        Ok(ImportReport {
            document_id,
            object_count,
            quad_count,
            validation_status: recorded.status,
            validation_issue_count: recorded.issue_count,
        })
    }
}

fn db_err<E: std::fmt::Display>(e: E) -> DomainError {
    DomainError::Database(e.to_string())
}

fn to_rdf_format(format: SerializationFormat) -> Result<RdfFormat, DomainError> {
    match format {
        SerializationFormat::Turtle => Ok(RdfFormat::Turtle),
        SerializationFormat::JsonLd => Ok(RdfFormat::JsonLd),
        SerializationFormat::RdfXml => Ok(RdfFormat::RdfXml),
        SerializationFormat::NTriples => Ok(RdfFormat::NTriples),
        other => Err(DomainError::InvalidInput(format!(
            "serialization format {other:?} is not supported by the upstream parser yet"
        ))),
    }
}

/// Minimal snapshot of the parsed RDF graph as a JSON array of triples,
/// suitable for storing as the lossless `raw_payload`.
fn triples_json_snapshot(doc: &Document) -> Vec<serde_json::Value> {
    use sbol::{Resource, Term};
    doc.rdf_graph()
        .triples()
        .iter()
        .map(|t| {
            let subject = match &t.subject {
                Resource::Iri(iri) => serde_json::json!({ "iri": iri.as_str() }),
                Resource::BlankNode(node) => serde_json::json!({ "blank": node.as_str() }),
                _ => serde_json::json!({ "blank": format!("{}", t.subject) }),
            };
            let object = match &t.object {
                Term::Resource(Resource::Iri(iri)) => serde_json::json!({ "iri": iri.as_str() }),
                Term::Resource(Resource::BlankNode(node)) => {
                    serde_json::json!({ "blank": node.as_str() })
                }
                Term::Literal(lit) => serde_json::json!({
                    "literal": lit.value(),
                    "datatype": lit.datatype().as_str(),
                    "language": lit.language(),
                }),
                _ => serde_json::Value::Null,
            };
            serde_json::json!({
                "s": subject,
                "p": t.predicate.as_str(),
                "o": object,
            })
        })
        .collect()
}
