use sbol::{Document, Iri, RdfFormat, UpgradeOptions};
use sbol_db_core::{
    DomainError, GraphId, ImportReport, IriString, NewGraph, SerializationFormat, Triple,
};
use sbol_db_rdf::{
    document_to_projections, document_to_summaries, document_to_triples, hash_bytes,
    rdf_graph_to_triples, GRAPH_IRI_PREFIX,
};

use crate::repo::{
    GraphRepository, NeighborhoodRepository, OntologyRepository, ProjectionEvent,
    ProjectionEventRepository, RecordedValidation, SbolObjectRepository, SequenceSearchRepository,
    TripleRepository, TypedProjectionCounts, TypedProjectionRepository, ValidationRepository,
};
use crate::PgPool;

pub struct SbolObjectService {
    pool: PgPool,
    graphs: GraphRepository,
    objects: SbolObjectRepository,
    triples: TripleRepository,
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
    pub namespace: Option<String>,
    pub source_uri: Option<String>,
    pub document_iri: Option<IriString>,
    pub created_by: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
}

/// How a Graph Store write combines with a graph's existing contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphWriteMode {
    /// `POST`: append to the graph (SynBioHub uploads submissions as a sequence
    /// of chunks POSTed to the same graph, so this must accumulate).
    Merge,
    /// `PUT`: replace the graph's entire contents.
    Replace,
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

    pub fn graphs(&self) -> &GraphRepository {
        &self.graphs
    }

    pub fn objects(&self) -> &SbolObjectRepository {
        &self.objects
    }

    pub fn triples(&self) -> &TripleRepository {
        &self.triples
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
        tx.commit().await.map_err(db_err)?;
        Ok(inserted)
    }

    /// Graph Store CRUD `DELETE`: drop a named graph entirely (its triples and
    /// `sbol_graphs` registry row). Returns the number of triples removed.
    pub async fn graph_store_clear(&self, graph: &str) -> Result<usize, DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let deleted = self.triples.clear_graph(&mut tx, Some(graph)).await?;
        self.triples.delete_graph(&mut tx, graph).await?;
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
        let doc = parse_import_document(&input)?;

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

        // A document is an `sbol3`-kind graph; `insert` creates that graph row
        // (id + `graph:document:{id}` IRI) and returns its id. We then write the
        // document's triples into that same graph — the graph owns them.
        let graph_id = self
            .graphs
            .insert(
                &mut *conn,
                NewGraph {
                    document_iri: input.document_iri.clone(),
                    name: input.name,
                    description: input.description,
                    serialization_format: input.format,
                    source_uri: input.source_uri,
                    content_hash: body_hash,
                    created_by: input.created_by,
                },
            )
            .await?;

        let graph_iri = IriString::unchecked(format!("{}{}", GRAPH_IRI_PREFIX, graph_id.as_uuid()));
        let triples = document_to_triples(&doc, &graph_iri);
        let triple_count = self
            .triples
            .insert_triples(&mut *conn, &triples, "sbol")
            .await?;

        // Derivation: the shared SBOL-view seam.
        let view = self
            .apply_sbol_view(&mut *conn, &doc, graph_id, &target_iri)
            .await?;

        self.projection
            .append(
                &mut *conn,
                ProjectionEvent {
                    event_type: "document_imported".to_owned(),
                    subject_iri: Some(IriString::unchecked(target_iri.clone())),
                    graph_iri: Some(graph_iri.clone()),
                    payload: serde_json::json!({
                        "graph_id": graph_id.as_uuid(),
                        "object_count": view.object_count,
                        "triple_count": triple_count,
                        "typed_counts": view.typed_counts,
                    }),
                },
            )
            .await?;

        Ok(ImportReport {
            graph_id,
            object_count: view.object_count,
            triple_count,
            validation_status: view.recorded.status,
            validation_issue_count: view.recorded.issue_count,
        })
    }

    /// Derive sbol-db's typed SBOL view from a parsed [`Document`] and upsert
    /// it: object summaries, typed projections, and a validation run, keyed by
    /// object IRI and attributed to `graph_id`.
    ///
    /// This is the single derivation seam, decoupled from how the triples
    /// arrived. SBOL document import passes a freshly parsed document; the
    /// verbatim-graph reprojection path (for SynBioHub's SBOL2 data)
    /// reconstructs a document from a graph's stored triples and calls the same
    /// method, so every ingest route produces the identical native view.
    async fn apply_sbol_view(
        &self,
        conn: &mut sqlx::PgConnection,
        doc: &Document,
        graph_id: GraphId,
        target_iri: &str,
    ) -> Result<SbolViewCounts, DomainError> {
        let summaries = document_to_summaries(doc);
        let object_count = summaries.len();
        for slice in &summaries {
            self.objects
                .upsert(&mut *conn, &slice.summary, Some(graph_id))
                .await?;
        }
        let typed_counts = self
            .typed
            .upsert_all(&mut *conn, &document_to_projections(doc))
            .await?;
        let recorded = self
            .validation
            .record_run(
                &mut *conn,
                target_iri,
                Some(graph_id),
                "sbol-rs",
                Some(sbol::SPEC_VERSION),
                "sbol3-3.1.0",
                &doc.validate(),
            )
            .await?;
        Ok(SbolViewCounts {
            object_count,
            typed_counts,
            recorded,
        })
    }
}

/// Counts plus the validation result from one
/// [`SbolObjectService::apply_sbol_view`] run.
struct SbolViewCounts {
    object_count: usize,
    typed_counts: TypedProjectionCounts,
    recorded: RecordedValidation,
}

fn db_err<E: std::fmt::Display>(e: E) -> DomainError {
    DomainError::Database(e.to_string())
}

fn parse_import_document(input: &ImportInput) -> Result<Document, DomainError> {
    match input.format {
        SerializationFormat::GenBank => {
            let namespace = conversion_namespace(input)?;
            let importer = sbol_genbank::GenbankImporter::new(namespace.as_str())
                .map_err(|e| DomainError::InvalidInput(e.to_string()))?;
            importer
                .read_str(&input.body)
                .map(|(document, _report)| document)
                .map_err(|e| DomainError::Parse(e.to_string()))
        }
        SerializationFormat::Fasta => {
            let namespace = conversion_namespace(input)?;
            let importer = sbol_fasta::FastaImporter::new(namespace.as_str())
                .map_err(|e| DomainError::InvalidInput(e.to_string()))?;
            importer
                .read_str(&input.body)
                .map(|(document, _report)| document)
                .map_err(|e| DomainError::Parse(e.to_string()))
        }
        format => {
            let rdf_format = to_rdf_format(format)?;
            if looks_like_sbol2(&input.body) {
                let mut options = UpgradeOptions::default();
                options.default_namespace = input
                    .namespace
                    .as_deref()
                    .map(|namespace| {
                        Iri::new(namespace.to_owned()).map_err(|e| {
                            DomainError::InvalidInput(format!(
                                "invalid import namespace `{namespace}`: {e}"
                            ))
                        })
                    })
                    .transpose()?;
                Document::upgrade_from_sbol2_with(&input.body, rdf_format, options)
                    .map(|(document, _report)| document)
                    .map_err(|e| DomainError::Parse(e.to_string()))
            } else {
                Document::read(&input.body, rdf_format)
                    .map_err(|e| DomainError::Parse(e.to_string()))
            }
        }
    }
}

fn looks_like_sbol2(body: &str) -> bool {
    body.contains("http://sbols.org/v2#") || body.contains("https://sbols.org/v2#")
}

fn conversion_namespace(input: &ImportInput) -> Result<Iri, DomainError> {
    let namespace = input
        .namespace
        .clone()
        .or_else(|| {
            input
                .document_iri
                .as_ref()
                .map(|iri| iri.as_str().to_owned())
        })
        .or_else(|| {
            input
                .source_uri
                .as_deref()
                .and_then(default_namespace_from_label)
        })
        .or_else(|| input.name.as_deref().and_then(default_namespace_from_label))
        .unwrap_or_else(|| format!("https://sbol-db.local/imports/{}", uuid::Uuid::new_v4()));
    Iri::new(namespace.clone()).map_err(|e| {
        DomainError::InvalidInput(format!("invalid import namespace `{namespace}`: {e}"))
    })
}

fn default_namespace_from_label(label: &str) -> Option<String> {
    let stem = std::path::Path::new(label)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(label);
    let segment = sanitize_namespace_segment(stem);
    (!segment.is_empty()).then(|| format!("https://sbol-db.local/imports/{segment}"))
}

fn sanitize_namespace_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut previous_was_sep = false;
    for ch in raw.chars() {
        let mapped = if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            Some(ch)
        } else if ch.is_ascii_whitespace() || matches!(ch, '.' | '/' | '\\' | ':') {
            Some('_')
        } else {
            None
        };
        if let Some(ch) = mapped {
            if ch == '_' {
                if previous_was_sep {
                    continue;
                }
                previous_was_sep = true;
            } else {
                previous_was_sep = false;
            }
            out.push(ch);
        }
    }
    out.trim_matches('_').to_owned()
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

#[cfg(test)]
mod tests {
    use super::*;

    const SBOL2_TURTLE: &str = r#"
@prefix sbol: <http://sbols.org/v2#> .
@prefix dcterms: <http://purl.org/dc/terms/> .
@prefix biopax: <http://www.biopax.org/release/biopax-level3.owl#> .
@prefix so: <https://identifiers.org/SO:> .

<https://example.org/lab/J23100/1>
    a sbol:ComponentDefinition ;
    sbol:persistentIdentity <https://example.org/lab/J23100> ;
    sbol:displayId "J23100" ;
    sbol:version "1" ;
    dcterms:title "Anderson promoter J23100" ;
    sbol:type biopax:Dna ;
    sbol:role so:0000167 ;
    sbol:sequence <https://example.org/lab/J23100_seq/1> .

<https://example.org/lab/J23100_seq/1>
    a sbol:Sequence ;
    sbol:persistentIdentity <https://example.org/lab/J23100_seq> ;
    sbol:displayId "J23100_seq" ;
    sbol:version "1" ;
    sbol:elements "ttgacggctagctcagtcctaggtacagtgctagc" ;
    sbol:encoding <http://www.chem.qmul.ac.uk/iubmb/misc/naseq.html> .
"#;

    const GENBANK: &str = r#"
LOCUS       BBa_B0034                 12 bp    DNA     linear       20-May-2026
DEFINITION  RBS (Elowitz 1999) -- defines RBS efficiency
ACCESSION   BBa_B0034
VERSION     BBa_B0034.1
FEATURES             Location/Qualifiers
     misc_feature    5..8
                     /label=conserved
ORIGIN
        1 aaagaggaga aa
//
"#;

    const FASTA: &str = r#"
>BBa_B0034 RBS
aaagaggagaaa
"#;

    fn input(body: &str, format: SerializationFormat, namespace: Option<&str>) -> ImportInput {
        ImportInput {
            body: body.to_owned(),
            format,
            namespace: namespace.map(str::to_owned),
            source_uri: Some("test://fixture".to_owned()),
            document_iri: None,
            created_by: None,
            name: None,
            description: None,
        }
    }

    #[test]
    fn parses_sbol2_rdf_by_upgrading_to_sbol3() {
        let document =
            parse_import_document(&input(SBOL2_TURTLE, SerializationFormat::Turtle, None))
                .expect("upgrade");

        assert_eq!(document.components().count(), 1);
        assert_eq!(document.sequences().count(), 1);
        assert_eq!(document.validate().errors().count(), 0);
    }

    #[test]
    fn parses_genbank_as_sbol3_document() {
        let document = parse_import_document(&input(GENBANK, SerializationFormat::GenBank, None))
            .expect("genbank import");

        assert_eq!(document.components().count(), 1);
        assert_eq!(document.sequences().count(), 1);
        assert_eq!(document.sequence_features().count(), 1);
        assert_eq!(document.validate().errors().count(), 0);
    }

    #[test]
    fn parses_fasta_as_sbol3_document() {
        let document = parse_import_document(&input(FASTA, SerializationFormat::Fasta, None))
            .expect("fasta import");

        assert_eq!(document.components().count(), 1);
        assert_eq!(document.sequences().count(), 1);
        assert_eq!(document.validate().errors().count(), 0);
    }
}
