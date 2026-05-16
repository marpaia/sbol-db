use sbol_db_core::{DomainError, SerializationFormat};
use sbol_db_postgres::QuadRepository;
use sbol_db_rdf::quads_to_rdf;

/// Fetch the subject's quads then re-serialize in the requested format.
pub async fn export_subject_rdf(
    quads: &QuadRepository,
    subject_iri: &str,
    format: SerializationFormat,
) -> Result<String, DomainError> {
    let quads = quads.quads_for_subject(subject_iri).await?;
    quads_to_rdf(&quads, format)
}
