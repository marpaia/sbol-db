//! Admin configuration sections: mail, theme, and user-signup policy, plus the
//! log tail.
//!
//! Each section is one key in the durable
//! [`ConfigStore`](sbol_db_storage::ConfigStore), the replacement for classic
//! SynBioHub's mutable `config.local.json`. A `GET` returns the stored value
//! (or `null` when unset); a `POST` persists the posted body through
//! [`ConfigService`](sbol_db_app::ConfigService), which re-checks the caller is
//! an administrator. The admin router already gates these, so the re-check is
//! defense in depth.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::{Extension, Json};
use serde_json::{json, Value};

use super::{config_err, parse_config_value, CurrentUser};
use crate::error::ApiError;
use crate::synbiohub::setup::is_first_launch;
use crate::AppState;

/// The config key holding the outgoing-mail settings (`fromAddress`,
/// `sendgridApiKey`).
const MAIL_KEY: &str = "mail";
/// The config key holding the theme / instance branding.
const THEME_KEY: &str = "theme";
/// The config key holding the user-signup policy.
const USERS_CONFIG_KEY: &str = "usersConfig";

/// Read the value stored under `key`, or JSON `null` when unset.
async fn get_section(state: &AppState, key: &str) -> Result<Json<Value>, ApiError> {
    let value = state.app.config_service().get(key).await?;
    Ok(Json(value.unwrap_or(Value::Null)))
}

/// Persist the posted body under `key`, gated on the caller being an admin.
async fn set_section(
    state: &AppState,
    user: &Option<sbol_db_core::User>,
    key: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Json<Value>, ApiError> {
    let Some(caller) = user.as_ref() else {
        return Err(ApiError::Unauthorized(
            "authentication is required".to_owned(),
        ));
    };
    let is_admin = caller.is_admin;
    let value = parse_config_value(headers, body)?;
    state
        .app
        .config_service()
        .set(is_admin, key, &value)
        .await
        .map_err(config_err)?;
    Ok(Json(json!({ "status": "ok" })))
}

/// `GET /admin/mail`.
pub async fn get_mail(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    get_section(&state, MAIL_KEY).await
}

/// `POST /admin/mail`.
pub async fn set_mail(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    set_section(&state, &user, MAIL_KEY, &headers, &body).await
}

/// `GET /admin/theme` — the instance branding and policy the SynBioHub UI reads
/// to render (name, URI prefix, signup/login policy) and to decide whether to
/// show the first-run setup wizard (`firstLaunch`). Classic composes this object
/// from its config; sbol-db composes it from its server config, then overlays
/// any stored `theme` section (custom name, colors) on top. `firstLaunch` is
/// true until the instance is provisioned through [`crate::synbiohub::setup`],
/// which creates the first administrator and records the branding.
pub async fn get_theme(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let first_launch = is_first_launch(&state).await?;
    let mut config = json!({
        "instanceName": "SynBioHub",
        "frontendURL": "",
        "instanceUrl": "",
        "uriPrefix": "http://synbiohub.org/",
        "frontPageText": "",
        "firstLaunch": first_launch,
        "altHome": "",
        "currentTheme": "",
        "themeParameters": [{ "name": "Base Color", "variable": "baseColor", "value": "#D25627" }],
        "showModuleInteractions": false,
        "removePublicEnabled": false,
        "allowPublicSignup": state.config.allow_public_signup,
        "requireLogin": false,
        "pluginsUseLocalCompose": false,
        "pluginLocalComposePrefix": "",
        "suppressInfoLogs": false,
        "suppressDebugLogs": false,
        "suppressWarningLogs": false,
        "suppressErrorLogs": false,
    });
    if let (Value::Object(base), Some(Value::Object(stored))) = (
        &mut config,
        state.app.config_service().get(THEME_KEY).await?,
    ) {
        for (key, value) in stored {
            base.insert(key, value);
        }
    }
    Ok(Json(config))
}

/// `POST /admin/theme`.
pub async fn set_theme(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    set_section(&state, &user, THEME_KEY, &headers, &body).await
}

/// `GET /admin/users`.
pub async fn get_users_config(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    get_section(&state, USERS_CONFIG_KEY).await
}

/// `POST /admin/users`.
pub async fn set_users_config(
    State(state): State<AppState>,
    Extension(CurrentUser(user)): Extension<CurrentUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    set_section(&state, &user, USERS_CONFIG_KEY, &headers, &body).await
}

/// `GET /admin/log` — the recent admin log lines. The classic action tails the
/// process log; the adapter returns the same JSON envelope.
pub async fn log() -> Json<Value> {
    Json(json!({ "entries": [] }))
}
