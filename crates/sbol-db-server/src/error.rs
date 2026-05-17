use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use sbol_db_core::DomainError;
use sbol_db_sparql::SparqlError;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    Domain(#[from] DomainError),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("{0}")]
    Sparql(SparqlError),
    #[error("request timed out")]
    Timeout,
}

impl From<SparqlError> for ApiError {
    fn from(err: SparqlError) -> Self {
        match err {
            SparqlError::Domain(d) => ApiError::Domain(d),
            SparqlError::Timeout => ApiError::Timeout,
            other => ApiError::Sparql(other),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, kind) = match &self {
            ApiError::Domain(DomainError::NotFound(_)) => (StatusCode::NOT_FOUND, "not_found"),
            ApiError::Domain(DomainError::InvalidInput(_)) => {
                (StatusCode::BAD_REQUEST, "invalid_input")
            }
            ApiError::Domain(DomainError::Parse(_)) => (StatusCode::BAD_REQUEST, "parse_error"),
            ApiError::Domain(DomainError::Iri(_)) => (StatusCode::BAD_REQUEST, "invalid_iri"),
            ApiError::Domain(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
            ApiError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            ApiError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            ApiError::Sparql(SparqlError::Parse(_)) => {
                (StatusCode::BAD_REQUEST, "sparql_parse_error")
            }
            ApiError::Sparql(SparqlError::UpdateNotAllowed) => {
                (StatusCode::BAD_REQUEST, "sparql_update_not_allowed")
            }
            ApiError::Sparql(SparqlError::QueryTooLarge) => {
                (StatusCode::PAYLOAD_TOO_LARGE, "sparql_query_too_large")
            }
            ApiError::Sparql(SparqlError::UnsupportedFormat(_)) => {
                (StatusCode::NOT_ACCEPTABLE, "sparql_unsupported_format")
            }
            ApiError::Sparql(_) => (StatusCode::INTERNAL_SERVER_ERROR, "sparql_error"),
            ApiError::Timeout => (StatusCode::GATEWAY_TIMEOUT, "timeout"),
        };
        let detail = self.to_string();
        if status.is_server_error() {
            tracing::error!(
                status = status.as_u16(),
                kind = kind,
                detail = %detail,
                "request failed"
            );
        }
        let body = Json(json!({
            "type": kind,
            "title": kind,
            "status": status.as_u16(),
            "detail": detail,
        }));
        (status, body).into_response()
    }
}
