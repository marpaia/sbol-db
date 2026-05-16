use sbol_db_core::DomainError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SparqlError {
    #[error("SPARQL parse error: {0}")]
    Parse(String),

    #[error("SPARQL Update is not allowed on this endpoint")]
    UpdateNotAllowed,

    #[error("SPARQL evaluation failed: {0}")]
    Evaluation(String),

    #[error("SPARQL serialization failed: {0}")]
    Serialization(String),

    #[error("query exceeded the configured timeout")]
    Timeout,

    #[error("query body exceeded the configured maximum size")]
    QueryTooLarge,

    #[error("unsupported result format for this query form: {0}")]
    UnsupportedFormat(String),

    #[error(transparent)]
    Domain(#[from] DomainError),

    #[error("task join error: {0}")]
    Join(String),
}
