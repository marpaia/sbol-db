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
    /// The requested feature is not available on the active storage backend
    /// (e.g. a Postgres-only lab page when running on SQLite).
    #[error("{0}")]
    Unavailable(String),
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
            ApiError::Sparql(SparqlError::Unsupported(_)) => {
                (StatusCode::NOT_IMPLEMENTED, "sparql_unsupported")
            }
            ApiError::Sparql(_) => (StatusCode::INTERNAL_SERVER_ERROR, "sparql_error"),
            ApiError::Timeout => (StatusCode::GATEWAY_TIMEOUT, "timeout"),
            ApiError::Unavailable(_) => (StatusCode::NOT_IMPLEMENTED, "backend_unsupported"),
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

#[cfg(test)]
mod tests {
    //! Exhaustive `ApiError -> HTTP status` mapping. If a new error variant
    //! lands without its branch in `IntoResponse`, the catch-all collapses
    //! it to 500 — these cases pin the deliberate mappings so that
    //! regression shows up here, not in production.

    use super::*;
    use sbol_db_core::IriValidationError;

    fn status_of(err: ApiError) -> StatusCode {
        err.into_response().status()
    }

    #[test]
    fn domain_not_found_is_404() {
        assert_eq!(
            status_of(ApiError::Domain(DomainError::NotFound("x".into()))),
            StatusCode::NOT_FOUND,
        );
    }

    #[test]
    fn domain_invalid_input_is_400() {
        assert_eq!(
            status_of(ApiError::Domain(DomainError::InvalidInput("x".into()))),
            StatusCode::BAD_REQUEST,
        );
    }

    #[test]
    fn domain_parse_is_400() {
        assert_eq!(
            status_of(ApiError::Domain(DomainError::Parse("x".into()))),
            StatusCode::BAD_REQUEST,
        );
    }

    #[test]
    fn domain_iri_is_400() {
        assert_eq!(
            status_of(ApiError::Domain(DomainError::Iri(
                IriValidationError::Empty
            ))),
            StatusCode::BAD_REQUEST,
        );
    }

    #[test]
    fn domain_internal_variants_are_500() {
        for variant in [
            DomainError::Serialization("x".into()),
            DomainError::Validation("x".into()),
            DomainError::Database("x".into()),
            DomainError::Io("x".into()),
        ] {
            assert_eq!(
                status_of(ApiError::Domain(variant)),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    }

    #[test]
    fn top_level_bad_request_is_400() {
        assert_eq!(
            status_of(ApiError::BadRequest("x".into())),
            StatusCode::BAD_REQUEST,
        );
    }

    #[test]
    fn top_level_not_found_is_404() {
        assert_eq!(
            status_of(ApiError::NotFound("x".into())),
            StatusCode::NOT_FOUND,
        );
    }

    #[test]
    fn sparql_parse_is_400() {
        assert_eq!(
            status_of(ApiError::Sparql(SparqlError::Parse("x".into()))),
            StatusCode::BAD_REQUEST,
        );
    }

    #[test]
    fn sparql_update_not_allowed_is_400() {
        assert_eq!(
            status_of(ApiError::Sparql(SparqlError::UpdateNotAllowed)),
            StatusCode::BAD_REQUEST,
        );
    }

    #[test]
    fn sparql_query_too_large_is_413() {
        assert_eq!(
            status_of(ApiError::Sparql(SparqlError::QueryTooLarge)),
            StatusCode::PAYLOAD_TOO_LARGE,
        );
    }

    #[test]
    fn sparql_unsupported_format_is_406() {
        assert_eq!(
            status_of(ApiError::Sparql(SparqlError::UnsupportedFormat("x".into()))),
            StatusCode::NOT_ACCEPTABLE,
        );
    }

    #[test]
    fn sparql_internal_variants_are_500() {
        for variant in [
            SparqlError::Evaluation("x".into()),
            SparqlError::Serialization("x".into()),
            SparqlError::Join("x".into()),
        ] {
            assert_eq!(
                status_of(ApiError::Sparql(variant)),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    }

    #[test]
    fn top_level_timeout_is_504() {
        assert_eq!(status_of(ApiError::Timeout), StatusCode::GATEWAY_TIMEOUT);
    }

    #[test]
    fn top_level_unavailable_is_501() {
        assert_eq!(
            status_of(ApiError::Unavailable("x".into())),
            StatusCode::NOT_IMPLEMENTED,
        );
    }

    /// `From<SparqlError> for ApiError` must hoist `Timeout` and `Domain`
    /// variants out of the `Sparql(...)` wrapper so they map to their
    /// canonical status codes (504 / domain-specific) rather than the
    /// catch-all 500.
    #[test]
    fn sparql_timeout_is_hoisted_to_504() {
        let api: ApiError = SparqlError::Timeout.into();
        assert_eq!(status_of(api), StatusCode::GATEWAY_TIMEOUT);
    }

    #[test]
    fn sparql_domain_iri_is_hoisted_to_400() {
        let api: ApiError = SparqlError::Domain(DomainError::Iri(IriValidationError::Empty)).into();
        assert_eq!(status_of(api), StatusCode::BAD_REQUEST);
    }
}
