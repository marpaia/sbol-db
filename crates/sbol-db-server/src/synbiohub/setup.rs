//! First-launch setup, classic SynBioHub's `/setup`.
//!
//! A fresh instance has no administrator; the SynBioHub UI detects this through
//! `firstLaunch` on `/admin/theme` and shows a setup wizard, which POSTs `/setup`
//! to create the first administrator and record the instance's branding/policy.
//! Setup runs once: it is refused once an administrator exists, so a seeded or
//! migrated instance that already has one is provisioned and browsable.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use sbol_db_app::Registration;
use serde::Deserialize;
use serde_json::json;
use sha3::{Digest, Sha3_256};
use tokio::sync::Mutex;

use super::auth::parse_body;
use crate::error::ApiError;
use crate::AppState;

const SETUP_TOKEN_HEADER: &str = "x-sbol-db-setup-token";
static SETUP_LOCK: Mutex<()> = Mutex::const_new(());

/// Whether the instance still needs first-launch setup: it has no administrator.
/// Read by [`super::admin`]'s theme endpoint as `firstLaunch`. Deriving this from
/// admin presence (rather than a stored marker) means a seeded or migrated
/// instance that already has an administrator is correctly not first-launch.
pub(super) async fn is_first_launch(state: &AppState) -> Result<bool, ApiError> {
    crate::instance::setup_required(state).await
}

/// The first-launch form the SynBioHub UI POSTs as JSON.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetupBody {
    instance_name: Option<String>,
    instance_url: Option<String>,
    uri_prefix: Option<String>,
    front_page_text: Option<String>,
    color: Option<String>,
    alt_home: Option<String>,
    #[serde(default)]
    allow_public_signup: Option<bool>,
    #[serde(default)]
    require_login: Option<bool>,
    user_name: Option<String>,
    user_full_name: Option<String>,
    user_email: Option<String>,
    affiliation: Option<String>,
    user_password: Option<String>,
    user_password_confirm: Option<String>,
    /// One-time bootstrap token accepted in the body for setup UIs that cannot
    /// set a custom header. It is never persisted.
    setup_token: Option<String>,
}

fn plain(status: StatusCode, message: &str) -> Response {
    (status, [(CONTENT_TYPE, "text/plain")], message.to_owned()).into_response()
}

/// `GET /setup` — the setup status while the instance is unprovisioned; once set
/// up, classic answers `409 Conflict`, so this does too.
pub async fn get_setup(State(state): State<AppState>) -> Result<Response, ApiError> {
    if is_first_launch(&state).await? {
        Ok(Json(json!({ "firstLaunch": true })).into_response())
    } else {
        Ok(plain(StatusCode::CONFLICT, "SBOL DB is already set up"))
    }
}

/// `POST /setup` — provision the instance: create the first administrator and
/// record the branding/policy. Refused once the instance is already set up.
pub async fn post_setup(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let form: SetupBody = parse_body(&headers, &body)?;
    let presented_token = headers
        .get(SETUP_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .or(form.setup_token.as_deref());
    if !valid_setup_token(state.config.setup_token_hash.as_ref(), presented_token) {
        return Ok(plain(
            StatusCode::UNAUTHORIZED,
            "a valid first-launch setup token is required",
        ));
    }

    let username = form
        .user_name
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest("userName is required".to_owned()))?;
    let email = form
        .user_email
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest("userEmail is required".to_owned()))?;
    let password = form
        .user_password
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest("userPassword is required".to_owned()))?;
    if let Some(confirm) = form.user_password_confirm.filter(|s| !s.is_empty()) {
        if confirm != password {
            return Err(ApiError::BadRequest("passwords do not match".to_owned()));
        }
    }
    let name = form
        .user_full_name
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| username.clone());

    // The production data-directory lock guarantees one server process, and
    // this mutex serializes concurrent requests inside it. Re-checking after the
    // lock closes the two-request race that could otherwise create two admins.
    let _setup_guard = SETUP_LOCK.lock().await;
    if !is_first_launch(&state).await? {
        return Ok(plain(StatusCode::FORBIDDEN, "SBOL DB is already set up"));
    }

    // Persist policy first. If this succeeds but account creation fails, the
    // instance remains first-launch and a corrected request can retry. The
    // inverse ordering could strand an administrator with missing policy.
    let color = form
        .color
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| crate::instance::SBOL_DB_ACCENT_COLOR.to_owned());
    let theme = json!({
        "instanceName": form.instance_name.filter(|s| !s.is_empty()).unwrap_or_else(|| crate::instance::DEFAULT_INSTANCE_NAME.to_owned()),
        "instanceUrl": form.instance_url.unwrap_or_default(),
        "uriPrefix": form.uri_prefix.filter(|s| !s.is_empty()).unwrap_or_else(|| crate::instance::DEFAULT_URI_PREFIX.to_owned()),
        "frontPageText": form.front_page_text.unwrap_or_default(),
        "themeParameters": [{ "name": "Base Color", "variable": "baseColor", "value": color }],
        "altHome": form.alt_home.unwrap_or_default(),
        "allowPublicSignup": form.allow_public_signup.unwrap_or(state.config.allow_public_signup),
        "requireLogin": form.require_login.unwrap_or(false),
    });
    state
        .app
        .config
        .set(crate::instance::THEME_KEY, &theme)
        .await?;

    // The first account is the administrator. Once this succeeds,
    // `setup_required` becomes false and the setup token can no longer be used.
    state
        .app
        .auth
        .register(Registration {
            username,
            name,
            email,
            affiliation: form.affiliation.filter(|s| !s.is_empty()),
            password,
            is_admin: true,
            is_curator: true,
            is_member: true,
        })
        .await?;

    Ok(plain(StatusCode::OK, "SBOL DB configured"))
}

fn valid_setup_token(expected: Option<&[u8; 32]>, presented: Option<&str>) -> bool {
    let Some(expected) = expected else {
        return true;
    };
    let Some(presented) = presented else {
        return false;
    };
    let actual: [u8; 32] = Sha3_256::digest(presented.as_bytes()).into();
    let mut difference = 0u8;
    for (left, right) in actual.iter().zip(expected.iter()) {
        difference |= left ^ right;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_token_hash_comparison_is_exact() {
        let expected: [u8; 32] = Sha3_256::digest(b"correct horse battery staple token").into();
        assert!(valid_setup_token(
            Some(&expected),
            Some("correct horse battery staple token")
        ));
        assert!(!valid_setup_token(Some(&expected), Some("wrong")));
        assert!(!valid_setup_token(Some(&expected), None));
        assert!(valid_setup_token(None, None));
    }
}
