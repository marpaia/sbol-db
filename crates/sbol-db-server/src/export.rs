use sbol_db_core::{DomainError, SerializationFormat};
use sbol_db_rdf::triples_to_rdf;
use sbol_db_storage::SbolStore;

/// Fetch the subject's triples and re-serialize them in the requested format
/// and SBOL version. The stored view is SBOL3; `sbol2` downgrades it on the way
/// out so a caller with no SBOL library still gets the version it asked for.
pub async fn export_subject_rdf(
    store: &dyn SbolStore,
    subject_iri: &str,
    format: SerializationFormat,
    sbol2: bool,
) -> Result<String, DomainError> {
    let triples = store.triples_for_subject(subject_iri).await?;
    if sbol2 {
        let ntriples = triples_to_rdf(&triples, SerializationFormat::NTriples)?;
        downgrade_sbol3_ntriples(&ntriples, format)
    } else {
        triples_to_rdf(&triples, format)
    }
}

/// Downgrade an SBOL3 document, given as N-Triples, to SBOL2 RDF in `format`.
/// Shared by the object and neighborhood RDF endpoints.
pub fn downgrade_sbol3_ntriples(
    ntriples: &str,
    format: SerializationFormat,
) -> Result<String, DomainError> {
    let document = sbol::Document::read(ntriples, sbol::RdfFormat::NTriples)
        .map_err(|e| DomainError::Parse(e.to_string()))?;
    let (graph, _report) = sbol::downgrade::sbol3_to_sbol2(&document, Default::default())
        .map_err(|e| DomainError::Serialization(e.to_string()))?;
    graph
        .write(rdf_format(format)?)
        .map_err(|e| DomainError::Serialization(e.to_string()))
}

/// Map a [`SerializationFormat`] to the RDF writer's format, rejecting the
/// non-RDF (GenBank/FASTA) formats that do not apply to a downgrade.
fn rdf_format(format: SerializationFormat) -> Result<sbol::RdfFormat, DomainError> {
    match format {
        SerializationFormat::Turtle => Ok(sbol::RdfFormat::Turtle),
        SerializationFormat::JsonLd => Ok(sbol::RdfFormat::JsonLd),
        SerializationFormat::RdfXml => Ok(sbol::RdfFormat::RdfXml),
        SerializationFormat::NTriples => Ok(sbol::RdfFormat::NTriples),
        other => Err(DomainError::InvalidInput(format!(
            "cannot serialize SBOL2 as {other:?}"
        ))),
    }
}
