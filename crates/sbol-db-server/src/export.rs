use sbol_db_core::{DomainError, SerializationFormat};
use sbol_db_rdf::triples_to_rdf;
use sbol_db_storage::SbolStore;

/// Fetch the subject's triples then re-serialize in the requested format.
pub async fn export_subject_rdf(
    store: &dyn SbolStore,
    subject_iri: &str,
    format: SerializationFormat,
) -> Result<String, DomainError> {
    let triples = store.triples_for_subject(subject_iri).await?;
    triples_to_rdf(&triples, format)
}
