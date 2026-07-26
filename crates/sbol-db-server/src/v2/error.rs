//! The V2 error envelope.
//!
//! Every V2 handler returns [`V2Error`], which renders a single consistent
//! JSON shape, `{ "error": { "code", "message", "status" } }`. The status and
//! machine `code` come from the native [`ApiError`](crate::error::ApiError)
//! mapping so the two surfaces never diverge on semantics; V2 only re-presents
//! them under a nested envelope.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use sbol_db_app::MutationError;
use sbol_db_core::DomainError;
use sbol_db_search_sdk::SearchError;
use sbol_db_sparql::SparqlError;
use serde_json::json;

use crate::error::ApiError;

/// A V2 error: an HTTP status, a stable machine `code`, and a human `message`.
/// Built from an [`ApiError`] (or a facade error via `?`).
#[derive(Debug)]
pub struct V2Error {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl From<ApiError> for V2Error {
    fn from(err: ApiError) -> Self {
        let (status, code) = err.status_and_code();
        Self {
            status,
            code,
            message: err.to_string(),
        }
    }
}

impl From<DomainError> for V2Error {
    fn from(err: DomainError) -> Self {
        ApiError::from(err).into()
    }
}

impl From<SparqlError> for V2Error {
    fn from(err: SparqlError) -> Self {
        ApiError::from(err).into()
    }
}

impl From<MutationError> for V2Error {
    fn from(err: MutationError) -> Self {
        ApiError::from(err).into()
    }
}

impl From<SearchError> for V2Error {
    fn from(err: SearchError) -> Self {
        let (status, code) = match &err {
            SearchError::InvalidRequest(_) => (StatusCode::BAD_REQUEST, "search_invalid_request"),
            SearchError::Unsupported(_) => (StatusCode::BAD_REQUEST, "search_unsupported"),
            SearchError::Configuration(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "search_unavailable")
            }
            SearchError::Backend(_) => (StatusCode::INTERNAL_SERVER_ERROR, "search_backend_error"),
            SearchError::Cancelled => (StatusCode::SERVICE_UNAVAILABLE, "search_cancelled"),
        };
        Self {
            status,
            code,
            message: err.to_string(),
        }
    }
}

impl IntoResponse for V2Error {
    fn into_response(self) -> Response {
        if self.status.is_server_error() {
            tracing::error!(
                status = self.status.as_u16(),
                code = self.code,
                detail = %self.message,
                "v2 request failed"
            );
        }
        let body = Json(json!({
            "error": {
                "code": self.code,
                "message": self.message,
                "status": self.status.as_u16(),
            }
        }));
        (self.status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use sbol_db_core::IriValidationError;

    async fn envelope(err: V2Error) -> (StatusCode, serde_json::Value) {
        let res = err.into_response();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), 64 * 1024).await.expect("body");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        (status, value)
    }

    #[tokio::test]
    async fn not_found_is_404_with_nested_envelope() {
        let err: V2Error = ApiError::NotFound("x".into()).into();
        let (status, body) = envelope(err).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "not_found");
        assert_eq!(body["error"]["status"], 404);
    }

    #[tokio::test]
    async fn invalid_input_is_400() {
        let err: V2Error = DomainError::InvalidInput("x".into()).into();
        let (status, body) = envelope(err).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "invalid_input");
    }

    #[tokio::test]
    async fn timeout_is_504() {
        let err: V2Error = ApiError::Timeout.into();
        let (status, body) = envelope(err).await;
        assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(body["error"]["code"], "timeout");
    }

    #[tokio::test]
    async fn sparql_domain_iri_is_hoisted_to_400() {
        let err: V2Error = SparqlError::Domain(DomainError::Iri(IriValidationError::Empty)).into();
        let (status, _body) = envelope(err).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn invalid_structured_search_is_400() {
        let err: V2Error = SearchError::InvalidRequest("bad cursor".to_owned()).into();
        let (status, body) = envelope(err).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "search_invalid_request");
    }
}
