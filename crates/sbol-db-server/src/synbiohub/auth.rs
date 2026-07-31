//! Classic SynBioHub auth routes: `/login`, `/logout`, `/register`,
//! `/profile`, `/resetPassword`, `/setNewPassword`.
//!
//! These reproduce classic SynBioHub's request and response shapes so its
//! clients (and pySBOL2's `PartShop`) authenticate unchanged. Bodies arrive as
//! `application/x-www-form-urlencoded` (classic) or JSON (API clients); the
//! response is Accept-negotiated, mirroring classic's `res.plainOrHtml`: an API
//! client (no `text/html`) gets a machine-readable body, a browser gets a
//! session cookie and a redirect.

use axum::body::{Body, Bytes};
use axum::extract::{Extension, State};
use axum::http::header::{ACCEPT, CONTENT_TYPE, LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, SecondsFormat, Utc};
use sbol_db_app::{PasswordReset, Registration};
use sbol_db_core::{DomainError, User};
use sbol_db_storage::NewJob;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;

use super::CurrentUser;
use crate::error::ApiError;
use crate::session::{login_cookie, logout_cookie, token_from_cookie};
use crate::AppState;

#[derive(Deserialize, Default)]
struct LoginBody {
    /// Matches either the account email or its username, as classic's login
    /// form does.
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    next: Option<String>,
}

/// `POST /login` — verify credentials and mint an API token. API clients get
/// the bare token as a `text/plain` body (classic's `res.plainOrHtml`
/// message); browsers get a session cookie and a 302 to `next`.
pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let form: LoginBody = parse_body(&headers, &body)?;
    let (Some(email), Some(password)) = (form.email, form.password) else {
        return Err(ApiError::BadRequest(
            "email and password are required".to_owned(),
        ));
    };
    let user = match state
        .app
        .auth
        .authenticate(&email, &password, &state.config.password_salt)
        .await
    {
        Ok(user) => user,
        Err(DomainError::Validation(_)) => return Ok(unauthorized("invalid credentials")),
        Err(err) => return Err(err.into()),
    };
    let token = state.app.auth.issue_token(user.id).await?;

    if wants_html(&headers) {
        let next = form
            .next
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "/".to_owned());
        Ok(browser_login(
            &token,
            &next,
            state.config.session_cookie_secure,
        ))
    } else {
        Ok(([(CONTENT_TYPE, "text/plain")], token).into_response())
    }
}

/// `POST /logout` — revoke the presented token (classic deletes it from its
/// in-memory map). Browsers additionally get their session cookie cleared and a
/// redirect home.
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(token) = headers
        .get("x-authorization")
        .and_then(|v| v.to_str().ok())
        .filter(|t| !t.is_empty())
        .or_else(|| token_from_cookie(&headers))
    {
        state.app.auth.revoke_token(token).await?;
    }
    if wants_html(&headers) {
        Ok(clear_session_redirect(
            "/",
            state.config.session_cookie_secure,
        ))
    } else {
        Ok(StatusCode::OK.into_response())
    }
}

#[derive(Deserialize, Default)]
struct RegisterBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    affiliation: Option<String>,
    /// Accept classic's `password1`/`password2` pair as well as a plain
    /// `password` field.
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    password1: Option<String>,
    #[serde(default)]
    password2: Option<String>,
    #[serde(default)]
    next: Option<String>,
}

/// `POST /register` — create an account when public signup is enabled,
/// otherwise `403`. New accounts are members, not admins or curators. API
/// clients get a `text/plain` confirmation; browsers redirect to `next`.
pub async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    if !crate::instance::public_signup_allowed(&state).await? {
        return Ok(forbidden("public signup is disabled"));
    }
    let form: RegisterBody = parse_body(&headers, &body)?;
    let name = required(form.name, "name")?;
    let username = required(form.username, "username")?;
    // Classic validates the username with `validator.isAlphanumeric`: non-empty
    // and ASCII letters/digits only (no underscores, spaces, or punctuation).
    // A drop-in must reject the same inputs with the same 400.
    if username.is_empty() || !username.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(ApiError::BadRequest(
            "Please enter a valid username".to_owned(),
        ));
    }
    let email = required(form.email, "email")?;
    let password = form
        .password1
        .or(form.password)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| ApiError::BadRequest("password is required".to_owned()))?;
    if let Some(confirm) = form.password2.filter(|p| !p.is_empty()) {
        if confirm != password {
            return Err(ApiError::BadRequest("passwords do not match".to_owned()));
        }
    }
    let registration = Registration {
        username,
        name,
        email,
        affiliation: form.affiliation.filter(|a| !a.is_empty()),
        password,
        is_admin: false,
        is_curator: false,
        is_member: true,
    };
    state.app.auth.register(registration).await?;

    if wants_html(&headers) {
        let next = form
            .next
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "/".to_owned());
        Ok(redirect(&next))
    } else {
        Ok((
            [(CONTENT_TYPE, "text/plain")],
            "User registered successfully",
        )
            .into_response())
    }
}

/// `GET /profile` — the authenticated caller's account as JSON. Anonymous
/// callers get `401`.
pub async fn get_profile(Extension(CurrentUser(user)): Extension<CurrentUser>) -> Response {
    match user {
        Some(user) => Json(profile_json(&user)).into_response(),
        None => unauthorized("authentication required"),
    }
}

#[derive(Deserialize, Default)]
struct ProfileUpdateBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    affiliation: Option<String>,
}

/// `POST /profile` — update the caller's display name and affiliation.
/// Anonymous callers get `401`.
pub async fn update_profile(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let Some(mut user) = user else {
        return Ok(unauthorized("authentication required"));
    };
    let patch: ProfileUpdateBody = parse_body(&headers, &body)?;
    if let Some(name) = patch.name.filter(|n| !n.is_empty()) {
        user.name = name;
    }
    if let Some(affiliation) = patch.affiliation {
        user.affiliation = Some(affiliation).filter(|a| !a.is_empty());
    }
    state.app.users.update_user(&user).await?;

    if wants_html(&headers) {
        Ok(redirect("/profile"))
    } else {
        Ok((
            [(CONTENT_TYPE, "text/plain")],
            "Profile updated successfully",
        )
            .into_response())
    }
}

#[derive(Deserialize, Default)]
struct ResetPasswordBody {
    /// Matches account email or username, as the reset form does.
    #[serde(default)]
    email: Option<String>,
}

/// `POST /resetPassword` — mint a single-use reset link and enqueue a
/// `send_email` job to deliver it (the mail handler is a later phase; the job
/// is a durable no-op until then). The response is identical whether or not the
/// address is registered, so it does not leak account existence.
pub async fn reset_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let form: ResetPasswordBody = parse_body(&headers, &body)?;
    let identifier = required(form.email, "email")?;
    if let Some(reset) = state.app.auth.reset_password(&identifier).await? {
        enqueue_reset_email(&state, &reset).await?;
    }
    Ok((
        [(CONTENT_TYPE, "text/plain")],
        "If that account exists, a password reset link has been sent.",
    )
        .into_response())
}

#[derive(Deserialize, Default)]
struct SetNewPasswordBody {
    /// The reset link delivered by `/resetPassword`. Accept the classic field
    /// name alongside the shorter aliases.
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    link: Option<String>,
    #[serde(default, rename = "resetPasswordLink")]
    reset_password_link: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    password1: Option<String>,
    #[serde(default)]
    password2: Option<String>,
}

/// `POST /setNewPassword` — consume a reset link and set the new argon2 hash.
/// An unrecognized or already-used link is a `400`.
pub async fn set_new_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let form: SetNewPasswordBody = parse_body(&headers, &body)?;
    let link = form
        .token
        .or(form.link)
        .or(form.reset_password_link)
        .filter(|l| !l.is_empty())
        .ok_or_else(|| ApiError::BadRequest("a reset token is required".to_owned()))?;
    let password = form
        .password1
        .or(form.password)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| ApiError::BadRequest("a new password is required".to_owned()))?;
    if let Some(confirm) = form.password2.filter(|p| !p.is_empty()) {
        if confirm != password {
            return Err(ApiError::BadRequest("passwords do not match".to_owned()));
        }
    }
    match state.app.auth.set_new_password(&link, &password).await {
        Ok(_) => Ok((
            [(CONTENT_TYPE, "text/plain")],
            "Password updated successfully.",
        )
            .into_response()),
        Err(DomainError::Validation(message)) => Ok((
            StatusCode::BAD_REQUEST,
            [(CONTENT_TYPE, "text/plain")],
            message,
        )
            .into_response()),
        Err(err) => Err(err.into()),
    }
}

/// Enqueue the `send_email` job carrying a password-reset link. The handler is
/// added in the mail phase; until then the job persists as a no-op record.
async fn enqueue_reset_email(state: &AppState, reset: &PasswordReset) -> Result<(), ApiError> {
    let payload = json!({
        "template": "reset_password",
        "to": reset.email,
        "username": reset.username,
        "reset_link": reset.reset_link,
    });
    state
        .app
        .jobs
        .enqueue(NewJob {
            kind: "send_email".to_owned(),
            payload,
            queue: None,
            priority: None,
            max_attempts: None,
            idempotency_key: None,
            available_at: None,
            parent_job_id: None,
            correlation_id: None,
        })
        .await?;
    Ok(())
}

/// The profile JSON classic returns to an API client, keyed with classic's
/// camelCase field names. `password` and `resetPasswordLink` are always blank,
/// matching classic: the account's credentials never travel in the profile
/// response even when a reset is outstanding. `createdAt`/`updatedAt` are the
/// account's real timestamps in classic's Sequelize wire format (RFC3339 with
/// millisecond precision and a `Z` suffix). `user_external_profiles` is the set
/// of linked external-identity profiles, empty because this instance has no
/// external-auth providers, exactly as classic returns for an unlinked account.
fn profile_json(user: &User) -> serde_json::Value {
    json!({
        "id": user.id,
        "username": user.username,
        "name": user.name,
        "email": user.email,
        "affiliation": user.affiliation.clone().unwrap_or_default(),
        "password": "",
        "graphUri": user.graph_uri,
        "isAdmin": user.is_admin,
        "isCurator": user.is_curator,
        "isMember": user.is_member,
        "resetPasswordLink": "",
        "createdAt": iso8601_millis(&user.created_at),
        "updatedAt": iso8601_millis(&user.updated_at),
        "user_external_profiles": [],
    })
}

/// A UTC timestamp in classic's Sequelize wire format: RFC3339 with millisecond
/// precision and a `Z` zone suffix (`2026-07-17T21:12:27.022Z`).
fn iso8601_millis(at: &DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Whether the client prefers HTML (a browser), selecting the cookie+redirect
/// variant of an Accept-negotiated route. API clients that omit `text/html` get
/// the machine-readable variant, matching classic's `res.plainOrHtml`.
fn wants_html(headers: &HeaderMap) -> bool {
    headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|accept| {
            accept
                .split(',')
                .any(|media| media.split(';').next().unwrap_or("").trim() == "text/html")
        })
        .unwrap_or(false)
}

/// The lowercased base media type of the request body.
fn content_type(headers: &HeaderMap) -> String {
    headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default()
}

/// Deserialize a request body as JSON when the `Content-Type` says so, else as
/// form-encoded. Classic posts `application/x-www-form-urlencoded`; API clients
/// often send JSON.
pub(super) fn parse_body<T: DeserializeOwned>(
    headers: &HeaderMap,
    body: &[u8],
) -> Result<T, ApiError> {
    if content_type(headers) == "application/json" {
        serde_json::from_slice(body)
            .map_err(|e| ApiError::BadRequest(format!("invalid JSON body: {e}")))
    } else {
        serde_urlencoded::from_bytes(body)
            .map_err(|e| ApiError::BadRequest(format!("invalid form body: {e}")))
    }
}

/// A required string field, rejecting an absent or blank value.
fn required(value: Option<String>, field: &str) -> Result<String, ApiError> {
    match value {
        Some(v) if !v.trim().is_empty() => Ok(v),
        _ => Err(ApiError::BadRequest(format!("{field} is required"))),
    }
}

fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(CONTENT_TYPE, "text/plain")],
        message.to_owned(),
    )
        .into_response()
}

fn forbidden(message: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        [(CONTENT_TYPE, "text/plain")],
        message.to_owned(),
    )
        .into_response()
}

/// A 302 redirect with no body.
fn redirect(location: &str) -> Response {
    Response::builder()
        .status(StatusCode::FOUND)
        .header(LOCATION, location)
        .body(Body::empty())
        .expect("valid redirect response")
}

/// A browser login: set the session cookie and 302 to `next`.
fn browser_login(token: &str, next: &str, secure: bool) -> Response {
    Response::builder()
        .status(StatusCode::FOUND)
        .header(SET_COOKIE, login_cookie(token, secure))
        .header(LOCATION, next)
        .body(Body::empty())
        .expect("valid login redirect response")
}

/// A browser logout: expire the session cookie and 302 to `next`.
fn clear_session_redirect(next: &str, secure: bool) -> Response {
    Response::builder()
        .status(StatusCode::FOUND)
        .header(SET_COOKIE, logout_cookie(secure))
        .header(LOCATION, next)
        .body(Body::empty())
        .expect("valid logout redirect response")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with(name: &str, value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
        headers
    }

    #[test]
    fn wants_html_detects_browser_accept() {
        assert!(wants_html(&headers_with(
            "accept",
            "text/html,application/xhtml+xml"
        )));
        assert!(!wants_html(&headers_with("accept", "text/plain")));
        assert!(!wants_html(&headers_with("accept", "application/json")));
        assert!(!wants_html(&HeaderMap::new()));
    }

    #[test]
    fn parse_body_reads_form_and_json() {
        let form = headers_with("content-type", "application/x-www-form-urlencoded");
        let parsed: LoginBody = parse_body(&form, b"email=a@b.c&password=pw").unwrap();
        assert_eq!(parsed.email.as_deref(), Some("a@b.c"));
        assert_eq!(parsed.password.as_deref(), Some("pw"));

        let json = headers_with("content-type", "application/json");
        let parsed: LoginBody = parse_body(&json, br#"{"email":"a@b.c","password":"pw"}"#).unwrap();
        assert_eq!(parsed.email.as_deref(), Some("a@b.c"));
    }

    #[test]
    fn parse_body_rejects_malformed_json() {
        let json = headers_with("content-type", "application/json");
        let result: Result<LoginBody, _> = parse_body(&json, b"{not json");
        assert!(result.is_err());
    }

    #[test]
    fn required_rejects_blank() {
        assert!(required(Some("  ".to_owned()), "name").is_err());
        assert!(required(None, "name").is_err());
        assert_eq!(required(Some("x".to_owned()), "name").unwrap(), "x");
    }
}
