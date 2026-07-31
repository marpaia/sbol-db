//! Same-origin browser sessions for the modern portal.
//!
//! The session cookie carries the same opaque token API clients use as a
//! bearer credential, but it is `HttpOnly` and therefore never appears in the
//! JSON response or frontend JavaScript. Login, resolution, and revocation all
//! delegate to the shared [`AuthService`](sbol_db_app::AuthService).

use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, PRAGMA, SET_COOKIE, VARY};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use sbol_db_core::{DomainError, User, UserId};
use serde::{Deserialize, Serialize};

use super::auth::{reject_cross_origin_browser, Identity, PresentedCredential};
use super::error::V2Error;
use super::util::{parse_json, required};
use crate::error::ApiError;
use crate::session::{login_cookie, logout_cookie};
use crate::AppState;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LoginRequest {
    /// Matches either username or email. `email` remains an accepted alias so
    /// callers can migrate directly from classic SynBioHub's form field.
    #[serde(alias = "email", alias = "username")]
    identifier: Option<String>,
    password: Option<String>,
}

#[derive(Debug, Serialize)]
struct SessionResponse {
    authenticated: bool,
    user: Option<SessionUser>,
}

#[derive(Debug, Serialize)]
struct SessionUser {
    id: UserId,
    username: String,
    name: String,
    email: String,
    affiliation: Option<String>,
    graph_uri: String,
    is_admin: bool,
    is_curator: bool,
    is_member: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl SessionResponse {
    fn from_user(user: Option<User>) -> Self {
        Self {
            authenticated: user.is_some(),
            user: user.map(SessionUser::from),
        }
    }
}

impl From<User> for SessionUser {
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            username: user.username,
            name: user.name,
            email: user.email,
            affiliation: user.affiliation,
            graph_uri: user.graph_uri,
            is_admin: user.is_admin,
            is_curator: user.is_curator,
            is_member: user.is_member,
            created_at: user.created_at,
            updated_at: user.updated_at,
        }
    }
}

/// `GET /api/v2/session` — the current safe browser/account projection. An
/// anonymous caller receives `200` with `authenticated: false` so portal
/// bootstrap does not use errors for ordinary logged-out state.
pub(super) async fn get(Extension(identity): Extension<Identity>) -> Response {
    session_response(SessionResponse::from_user(identity.0))
}

/// `POST /api/v2/session` — verify credentials, mint a token, and establish an
/// `HttpOnly` same-origin session. The token is intentionally absent from the
/// response body; API clients can continue to obtain a bearer token from the
/// compatibility login endpoint.
pub(super) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, V2Error> {
    reject_cross_origin_browser(&headers)?;
    let request: LoginRequest = parse_json(&body)?;
    let identifier = required(request.identifier, "identifier")?;
    let password = required(request.password, "password")?;
    let user = match state
        .app
        .auth
        .authenticate(&identifier, &password, &state.config.password_salt)
        .await
    {
        Ok(user) => user,
        Err(DomainError::Validation(_)) => {
            return Err(V2Error::from(ApiError::Unauthorized(
                "invalid credentials".to_owned(),
            )))
        }
        Err(error) => return Err(error.into()),
    };
    let token = state.app.auth.issue_token(user.id).await?;
    let mut response = session_response(SessionResponse::from_user(Some(user)));
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&login_cookie(&token, state.config.session_cookie_secure))
            .expect("opaque UUID token always forms a valid cookie header"),
    );
    Ok(response)
}

/// `DELETE /api/v2/session` — revoke the credential selected by the auth
/// middleware and expire the browser cookie. Logout is idempotent: anonymous
/// and stale sessions also receive `204` plus the clearing cookie.
pub(super) async fn delete(
    State(state): State<AppState>,
    Extension(credential): Extension<PresentedCredential>,
) -> Result<Response, V2Error> {
    if let Some(token) = credential.token() {
        state.app.auth.revoke_token(token).await?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    no_store(response.headers_mut());
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&logout_cookie(state.config.session_cookie_secure))
            .expect("static cookie attributes form a valid header"),
    );
    Ok(response)
}

fn session_response(payload: SessionResponse) -> Response {
    let mut response = Json(payload).into_response();
    no_store(response.headers_mut());
    response
}

fn no_store(headers: &mut HeaderMap) {
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(VARY, HeaderValue::from_static("Cookie, Authorization"));
}
