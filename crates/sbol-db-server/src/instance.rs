//! Deployment identity and public policy shared by the V1 and V2 adapters.
//!
//! Classic SynBioHub exposes this material as the broad `/admin/theme` JSON
//! object. The native SBOL DB application needs a smaller, explicitly public
//! bootstrap contract. Both representations are derived here from the same
//! stored settings so setup and registration policy cannot silently disagree.
//! Legacy theme colors remain a compatibility concern and do not style the
//! native SBOL DB design system.

use serde_json::{json, Value};

use crate::error::ApiError;
use crate::AppState;

pub(crate) const THEME_KEY: &str = "theme";
pub(crate) const DEFAULT_INSTANCE_NAME: &str = "SBOL DB";
pub(crate) const DEFAULT_URI_PREFIX: &str = "http://localhost:8888/";
pub(crate) const SBOL_DB_ACCENT_COLOR: &str = "#21837F";

/// The safe, typed subset of instance configuration a public UI may consume.
#[derive(Clone, Debug)]
pub(crate) struct PublicInstanceSettings {
    pub name: String,
    pub instance_url: String,
    pub uri_prefix: String,
    pub front_page_text: String,
    pub allow_public_signup: bool,
    pub require_login: bool,
    pub setup_required: bool,
}

/// Whether the instance still needs first-launch setup. Admin presence is the
/// durable source of truth, so a seeded or migrated instance is already set up.
pub(crate) async fn setup_required(state: &AppState) -> Result<bool, ApiError> {
    Ok(!state.app.auth.any_admin().await?)
}

/// Build the legacy theme representation, overlaying persisted deployment
/// settings on runtime defaults. `firstLaunch` is written after the overlay
/// because it is derived state and must not be forgeable by stale stored data.
pub(crate) async fn legacy_theme(state: &AppState) -> Result<Value, ApiError> {
    let first_launch = setup_required(state).await?;
    let mut config = json!({
        "instanceName": DEFAULT_INSTANCE_NAME,
        "frontendURL": "",
        "instanceUrl": "",
        "uriPrefix": DEFAULT_URI_PREFIX,
        "frontPageText": "",
        "firstLaunch": first_launch,
        "altHome": "",
        "currentTheme": "",
        "themeParameters": [{ "name": "Base Color", "variable": "baseColor", "value": SBOL_DB_ACCENT_COLOR }],
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
        base.insert("firstLaunch".to_owned(), Value::Bool(first_launch));
    }
    Ok(config)
}

/// Load the public bootstrap subset with type-safe fallbacks for malformed or
/// older stored theme documents.
pub(crate) async fn public_settings(state: &AppState) -> Result<PublicInstanceSettings, ApiError> {
    let theme = legacy_theme(state).await?;
    let text = |key: &str, fallback: &str| {
        theme
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or(fallback)
            .to_owned()
    };
    Ok(PublicInstanceSettings {
        name: text("instanceName", DEFAULT_INSTANCE_NAME),
        instance_url: text("instanceUrl", ""),
        uri_prefix: text("uriPrefix", DEFAULT_URI_PREFIX),
        front_page_text: text("frontPageText", ""),
        allow_public_signup: theme
            .get("allowPublicSignup")
            .and_then(Value::as_bool)
            .unwrap_or(state.config.allow_public_signup),
        require_login: theme
            .get("requireLogin")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        setup_required: theme
            .get("firstLaunch")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    })
}

/// The effective self-service registration policy. This is shared with the V1
/// registration handler so the public bootstrap contract reports what the
/// server actually enforces.
pub(crate) async fn public_signup_allowed(state: &AppState) -> Result<bool, ApiError> {
    Ok(public_settings(state).await?.allow_public_signup)
}
