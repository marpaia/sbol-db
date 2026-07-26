use thiserror::Error;

/// A strategy or shared search-runtime failure.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum SearchError {
    #[error("invalid search request: {0}")]
    InvalidRequest(String),
    #[error("search capability is unavailable: {0}")]
    Unsupported(String),
    #[error("search is not configured correctly: {0}")]
    Configuration(String),
    #[error("search backend failed: {0}")]
    Backend(String),
    #[error("search was cancelled")]
    Cancelled,
}

/// A vector search or vector index lifecycle failure.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum VectorError {
    #[error("invalid vector request: {0}")]
    InvalidRequest(String),
    #[error("vector capability is unavailable: {0}")]
    Unsupported(String),
    #[error("vector backend is not configured correctly: {0}")]
    Configuration(String),
    #[error("vector backend failed: {0}")]
    Backend(String),
}

impl From<VectorError> for SearchError {
    fn from(value: VectorError) -> Self {
        SearchError::Backend(value.to_string())
    }
}

/// Startup-time rejection of an invalid plugin registry.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum RegistrationError {
    #[error("{kind} identifier cannot be empty")]
    EmptyId { kind: &'static str },
    #[error("duplicate {kind} registration: {id}")]
    Duplicate { kind: &'static str, id: String },
}
