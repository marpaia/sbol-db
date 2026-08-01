//! Bearer-token and same-origin browser-session authentication for V2.
//!
//! [`attach_identity`] prefers an `Authorization: Bearer <token>` header and,
//! when no Authorization header is present, accepts the shared `HttpOnly`
//! browser cookie. Both resolve through the facade's
//! [`AuthService`](sbol_db_app::AuthService) to a [`User`]. Authentication is
//! tolerant when public browsing is enabled: a missing, malformed, stale, or
//! unrecognized token is anonymous rather than rejected, and an anonymous
//! caller is scoped to the public graph by [`AclService`](sbol_db_app::AclService).
//! An instance configured with `requireLogin` rejects anonymous resource
//! requests while keeping bootstrap, instance, session, and API-doc routes
//! public.
//!
//! Cookie-authenticated unsafe requests additionally require a same-origin
//! browser signal. This keeps the convenience of a credential the SPA cannot
//! read from JavaScript without turning V2 mutations into CSRF targets.

use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, HOST, ORIGIN};
use axum::http::{HeaderMap, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sbol_db_core::User;
use sbol_db_sparql::GraphScope;
use url::Url;

use crate::error::ApiError;
use crate::session::token_from_cookie;
use crate::v2::error::V2Error;
use crate::AppState;

/// The account a V2 request authenticates as, or anonymous (`None`).
#[derive(Clone, Debug, Default)]
pub struct Identity(pub Option<User>);

/// Where the credential attached to this request came from. The plaintext
/// token is retained only for the request lifetime so logout can revoke it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum CredentialSource {
    #[default]
    None,
    Bearer,
    Cookie,
}

/// The credential selected by the middleware. `Debug` deliberately redacts
/// the plaintext token so request diagnostics cannot leak it.
#[derive(Clone, Default)]
pub(super) struct PresentedCredential {
    token: Option<String>,
    source: CredentialSource,
}

impl std::fmt::Debug for PresentedCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PresentedCredential")
            .field("token", &self.token.as_ref().map(|_| "[REDACTED]"))
            .field("source", &self.source)
            .finish()
    }
}

impl PresentedCredential {
    pub(super) fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }
}

/// Resolve the caller's identity and attach it to the request. An absent or
/// unrecognized token yields an anonymous [`Identity`]. A valid cookie may be
/// rejected only when it is used on an unsafe cross-origin request.
pub async fn attach_identity(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let credential = presented_credential(req.headers());
    let user = match credential.token() {
        Some(token) => resolve_user(&state, token).await,
        None => None,
    };
    if user.is_some()
        && credential.source == CredentialSource::Cookie
        && is_unsafe(req.method())
        && !same_origin(req.headers())
    {
        return V2Error::from(ApiError::Forbidden(
            "cookie-authenticated mutations require a same-origin request".to_owned(),
        ))
        .into_response();
    }
    if user.is_none() && !is_public_bootstrap_path(req.uri().path()) {
        match crate::instance::public_settings(&state).await {
            Ok(settings) if settings.require_login => {
                return V2Error::from(ApiError::Unauthorized(
                    "login is required by this instance".to_owned(),
                ))
                .into_response();
            }
            Ok(_) => {}
            Err(error) => return V2Error::from(error).into_response(),
        }
    }
    req.extensions_mut().insert(Identity(user));
    req.extensions_mut().insert(credential);
    next.run(req).await
}

/// Reject an explicitly cross-origin browser request. Used by login itself,
/// which has no authenticated cookie yet and therefore cannot rely on the
/// middleware's cookie-mutation guard. Requests without browser origin
/// metadata remain available to non-browser API clients.
pub(super) fn reject_cross_origin_browser(headers: &HeaderMap) -> Result<(), V2Error> {
    let has_origin = headers.contains_key(ORIGIN);
    let fetch_site = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok());
    if (has_origin && !same_origin(headers)) || fetch_site == Some("cross-site") {
        return Err(V2Error::from(ApiError::Forbidden(
            "browser session requests must be same-origin".to_owned(),
        )));
    }
    Ok(())
}

/// The authenticated caller, or a `403` for an anonymous request. Mutating
/// verbs require an identity; matching the V1 adapter, a missing resource
/// credential is `Forbidden` rather than `401` (invalid credentials are `401`
/// only at the explicit session-create endpoint).
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

/// Prefer Authorization whenever the header is present, even if it is
/// malformed. This prevents an invalid explicit bearer credential from
/// silently falling through to ambient cookie authority.
fn presented_credential(headers: &HeaderMap) -> PresentedCredential {
    if let Some(header) = headers.get(AUTHORIZATION) {
        return PresentedCredential {
            token: header
                .to_str()
                .ok()
                .and_then(bearer_token)
                .map(str::to_owned),
            source: CredentialSource::Bearer,
        };
    }
    match token_from_cookie(headers) {
        Some(token) => PresentedCredential {
            token: Some(token.to_owned()),
            source: CredentialSource::Cookie,
        },
        None => PresentedCredential::default(),
    }
}

fn is_unsafe(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn is_public_bootstrap_path(path: &str) -> bool {
    let path = path.strip_prefix("/api/v2").unwrap_or(path);
    matches!(
        path,
        "" | "/" | "/instance" | "/session" | "/openapi.json" | "/docs"
    )
}

/// Compare the browser's `Origin` authority with the request `Host`. Scheme is
/// intentionally not compared because TLS is commonly terminated by a reverse
/// proxy before the request reaches sbol-db; host and effective port remain the
/// stable same-origin boundary visible on both sides.
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(ORIGIN).and_then(|value| value.to_str().ok()) else {
        return headers
            .get("sec-fetch-site")
            .and_then(|value| value.to_str().ok())
            == Some("same-origin");
    };
    let Some(host) = headers.get(HOST).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let Ok(origin) = Url::parse(origin) else {
        return false;
    };
    let Ok(authority) = host.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    if origin.host_str().map(str::to_ascii_lowercase) != Some(authority.host().to_ascii_lowercase())
    {
        return false;
    }
    let request_port = authority.port_u16().or_else(|| match origin.scheme() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    });
    origin.port_or_known_default() == request_port
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

    #[test]
    fn bearer_header_has_precedence_over_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "Basic nope".parse().unwrap());
        headers.insert(
            axum::http::header::COOKIE,
            "sbol-db-token=cookie-token".parse().unwrap(),
        );
        let credential = presented_credential(&headers);
        assert_eq!(credential.source, CredentialSource::Bearer);
        assert_eq!(credential.token(), None);
    }

    #[test]
    fn reads_cookie_when_authorization_is_absent() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            "theme=dark; sbol-db-token=cookie-token".parse().unwrap(),
        );
        let credential = presented_credential(&headers);
        assert_eq!(credential.source, CredentialSource::Cookie);
        assert_eq!(credential.token(), Some("cookie-token"));
    }

    #[test]
    fn origin_must_match_host_and_effective_port() {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, "example.org".parse().unwrap());
        headers.insert(ORIGIN, "https://example.org".parse().unwrap());
        assert!(same_origin(&headers));

        headers.insert(ORIGIN, "https://evil.example".parse().unwrap());
        assert!(!same_origin(&headers));

        headers.insert(HOST, "example.org:8443".parse().unwrap());
        headers.insert(ORIGIN, "https://example.org:8443".parse().unwrap());
        assert!(same_origin(&headers));
    }

    #[test]
    fn only_bootstrap_contracts_bypass_require_login() {
        for path in [
            "/",
            "/instance",
            "/session",
            "/openapi.json",
            "/docs",
            "/api/v2/instance",
        ] {
            assert!(is_public_bootstrap_path(path), "{path}");
        }
        for path in ["/search", "/objects", "/api/v2/search"] {
            assert!(!is_public_bootstrap_path(path), "{path}");
        }
    }
}
