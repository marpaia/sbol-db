use thiserror::Error;

use crate::iri::IriValidationError;

/// Top-level error surfaced by domain services. Variants are deliberately
/// coarse — repositories convert sqlx errors into `Database`, parser errors
/// into `Parse`, etc., so consumers can branch on the failure mode without
/// depending on lower-level crate error types.
#[derive(Debug, Error)]
pub enum DomainError {
    #[error("invalid IRI: {0}")]
    Iri(#[from] IriValidationError),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("validation error: {0}")]
    Validation(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("database error: {0}")]
    Database(String),

    #[error("io error: {0}")]
    Io(String),
}

impl From<serde_json::Error> for DomainError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serialization(value.to_string())
    }
}

impl From<std::io::Error> for DomainError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}
