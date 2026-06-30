//! Parse an import body into an `sbol::Document` and derive the full set of
//! records a backend must persist for it.

use sbol::{Document, Iri, RdfFormat, UpgradeOptions, ValidationReport};
use sbol_db_core::{
    DomainError, GraphId, IriString, NewGraph, ObjectSummary, SerializationFormat, Triple,
    TypedProjections, ValidationStatus,
};
use sbol_db_rdf::{
    document_to_projections, document_to_summaries, document_to_triples, hash_bytes,
    GRAPH_IRI_PREFIX,
};
use sbol_db_storage::ImportInput;

/// Everything one document import must write, derived from the parsed body
/// with no database involved. A backend persists these atomically in its own
/// idiom: register the graph, insert the triples, upsert the object summaries
/// and typed projections, and record the validation run.
///
/// The `graph_id` (and the `graph_iri` derived from it) is minted here rather
/// than by the backend, so the triples and summaries can be attributed to the
/// graph before any write happens.
pub struct ImportPlan {
    /// Surrogate id of the document's graph; its IRI is `graph:document:{id}`.
    pub graph_id: GraphId,
    /// Registry row for the document's `sbol3`-kind graph.
    pub new_graph: NewGraph,
    /// `graph:document:{graph_id}`; the owner of every triple in `triples`.
    pub graph_iri: IriString,
    /// IRI the validation run is recorded against (the document's namespace).
    pub target_iri: String,
    /// The document's triples, each tagged with `graph_iri`.
    pub triples: Vec<Triple>,
    /// Per-object derived-view summaries feeding `sbol_objects`.
    pub summaries: Vec<ObjectSummary>,
    /// Typed projections (components, sequences, features, ...).
    pub projections: TypedProjections,
    /// The validation report produced by the SBOL validator. Backends that
    /// store individual findings consume this; the summary status and count
    /// below are precomputed for the [`ImportReport`](sbol_db_core::ImportReport).
    pub validation: ValidationReport,
    /// Summary classification of `validation`.
    pub validation_status: ValidationStatus,
    /// Number of validation issues in `validation`.
    pub validation_issue_count: usize,
}

/// Parse `input` and derive its [`ImportPlan`]. Pure: no I/O beyond parsing.
pub fn build_import_plan(input: &ImportInput) -> Result<ImportPlan, DomainError> {
    let doc = parse_import_document(input)?;

    let content_hash = hash_bytes(input.body.as_bytes());

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

    let graph_id = GraphId(uuid::Uuid::new_v4());
    let graph_iri = IriString::unchecked(format!("{}{}", GRAPH_IRI_PREFIX, graph_id.as_uuid()));

    let triples = document_to_triples(&doc, &graph_iri);
    let summaries = document_to_summaries(&doc)
        .into_iter()
        .map(|slice| slice.summary)
        .collect();
    let projections = document_to_projections(&doc);
    let validation = doc.validate();
    let validation_status = classify(&validation);
    let validation_issue_count = validation.issues().len();

    let new_graph = NewGraph {
        document_iri: input.document_iri.clone(),
        name: input.name.clone(),
        description: input.description.clone(),
        serialization_format: input.format,
        source_uri: input.source_uri.clone(),
        content_hash,
        created_by: input.created_by.clone(),
    };

    Ok(ImportPlan {
        graph_id,
        new_graph,
        graph_iri,
        target_iri,
        triples,
        summaries,
        projections,
        validation,
        validation_status,
        validation_issue_count,
    })
}

/// Classify a validation report into a summary status: any error fails, any
/// warning warns, otherwise it passes.
fn classify(report: &ValidationReport) -> ValidationStatus {
    if report.has_errors() {
        ValidationStatus::Failed
    } else if report.warnings().next().is_some() {
        ValidationStatus::Warning
    } else {
        ValidationStatus::Passed
    }
}

/// Parse an import body into an `sbol::Document`, dispatching on its declared
/// format. GenBank and FASTA are converted into SBOL3; RDF bodies are read
/// directly, upgrading SBOL2 to SBOL3 when the body uses the SBOL2 vocabulary.
pub fn parse_import_document(input: &ImportInput) -> Result<Document, DomainError> {
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

/// Map a [`SerializationFormat`] to the upstream parser's [`RdfFormat`].
/// Errors for formats the parser does not accept (GenBank/FASTA are handled
/// before this point by their dedicated importers).
pub fn to_rdf_format(format: SerializationFormat) -> Result<RdfFormat, DomainError> {
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

    #[test]
    fn import_plan_attributes_triples_and_summaries_to_one_graph() {
        let plan = build_import_plan(&input(SBOL2_TURTLE, SerializationFormat::Turtle, None))
            .expect("plan");

        // Every triple is owned by the document's graph.
        assert!(!plan.triples.is_empty());
        assert!(plan
            .triples
            .iter()
            .all(|t| t.graph_iri.as_ref() == Some(&plan.graph_iri)));

        // The component and its sequence both project to object summaries.
        assert_eq!(plan.summaries.len(), 2);
        assert_eq!(plan.projections.components.len(), 1);
        assert_eq!(plan.projections.sequences.len(), 1);
        assert_eq!(plan.validation.errors().count(), 0);
    }
}
