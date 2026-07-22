//! Bearer-token authentication for the V2 surface.
//!
//! [`attach_identity`] reads an `Authorization: Bearer <token>` header,
//! resolves it through the facade's [`AuthService`](sbol_db_app::AuthService)
//! to a [`User`], and attaches the result as an [`Identity`] extension every
//! V2 handler reads. Authentication is tolerant, matching the visibility a V1
//! client gets: a missing, malformed, or unrecognized token is anonymous
//! rather than rejected, and an anonymous caller is scoped to the public graph
//! by [`AclService`](sbol_db_app::AclService).
//!
//! Token transport is the idiomatic `Authorization: Bearer` header, not the
//! `X-authorization` header the V1 adapter carries.

use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::Response;
use sbol_db_core::User;
use sbol_db_sparql::GraphScope;

use crate::error::ApiError;
use crate::v2::error::V2Error;
use crate::AppState;

/// The account a V2 request authenticates as, or anonymous (`None`).
#[derive(Clone, Debug, Default)]
pub struct Identity(pub Option<User>);

/// Resolve the caller's identity from the `Authorization: Bearer` header and
/// attach it to the request. An absent or unrecognized token yields an
/// anonymous [`Identity`]; the request is never rejected here.
pub async fn attach_identity(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let token = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(bearer_token)
        .map(str::to_owned);
    let user = match token {
        Some(token) => resolve_user(&state, &token).await,
        None => None,
    };
    req.extensions_mut().insert(Identity(user));
    next.run(req).await
}

/// The authenticated caller, or a `403` for an anonymous request. Mutating
/// verbs require an identity; matching the V1 adapter, a missing credential is
/// `Forbidden` rather than `401` (this surface never issues `401` — an
/// unrecognized token resolves to anonymous, not rejected).
pub fn require_user(identity: &Identity) -> Result<User, V2Error> {
    identity
        .0
        .clone()
        .ok_or_else(|| V2Error::from(ApiError::Forbidden("authentication is required".to_owned())))
}

/// The caller's authorized graph scope, derived from their identity. Anonymous
/// callers see the public graph alone.
pub async fn scope_for(state: &AppState, identity: &Identity) -> Result<GraphScope, V2Error> {
    let user_graph = identity.0.as_ref().map(|u| u.graph_uri.as_str());
    Ok(state.app.acl_service.compute_scope(user_graph).await?)
}

/// Extract the token from an `Authorization` header value, requiring the
/// `Bearer` scheme (case-insensitive) and a non-empty token.
fn bearer_token(header: &str) -> Option<&str> {
    let (scheme, token) = header.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") {
        let token = token.trim();
        (!token.is_empty()).then_some(token)
    } else {
        None
    }
}

/// Resolve a plaintext bearer token to the account it authenticates, tolerating
/// a stale or bogus token (which resolves to `None`).
async fn resolve_user(state: &AppState, token: &str) -> Option<User> {
    let user_id = state.app.auth.resolve_token(token).await.ok().flatten()?;
    state.app.users.get_by_id(user_id).await.ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bearer_case_insensitively() {
        assert_eq!(bearer_token("Bearer abc123"), Some("abc123"));
        assert_eq!(bearer_token("bearer abc123"), Some("abc123"));
        assert_eq!(bearer_token("BEARER   abc123  "), Some("abc123"));
    }

    #[test]
    fn rejects_non_bearer_and_empty() {
        assert_eq!(bearer_token("Basic abc123"), None);
        assert_eq!(bearer_token("Bearer "), None);
        assert_eq!(bearer_token("abc123"), None);
    }
}
