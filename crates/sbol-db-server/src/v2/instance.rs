//! Public instance bootstrap contract for the SBOL DB application.
//!
//! This is deliberately narrower than classic `/admin/theme`: it exposes only
//! deployment identity, public access policy, setup state, and capability flags
//! the UI needs before it has a session. Legacy visual-theme settings, mail
//! credentials, plugin configuration, and every other admin-only setting remain
//! outside the response.

use axum::extract::State;
use axum::http::header::CACHE_CONTROL;
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;

use crate::v2::error::V2Error;
use crate::AppState;

#[derive(Debug, Serialize)]
pub(super) struct InstanceResponse {
    name: String,
    instance_url: String,
    uri_prefix: String,
    /// Stored front-page prose. Clients must render it as untrusted content.
    front_page_text: String,
    setup_required: bool,
    policies: InstancePolicies,
    capabilities: InstanceCapabilities,
}

#[derive(Debug, Serialize)]
struct InstancePolicies {
    allow_public_signup: bool,
    require_login: bool,
}

#[derive(Debug, Serialize)]
struct InstanceCapabilities {
    browser_sessions: bool,
    legacy_api: bool,
    structured_search: bool,
    sequence_search: bool,
    data_lab: bool,
    sql_console: bool,
}

/// `GET /api/v2/instance` — public deployment identity, access policy,
/// first-launch state, and feature discovery for the root application.
pub(super) async fn get(State(state): State<AppState>) -> Result<impl IntoResponse, V2Error> {
    let settings = crate::instance::public_settings(&state).await?;
    #[cfg(feature = "lab")]
    let data_lab = state.config.lab_enabled;
    #[cfg(not(feature = "lab"))]
    let data_lab = false;
    #[cfg(feature = "lab")]
    let sql_console = data_lab && state.sql_console.is_some();
    #[cfg(not(feature = "lab"))]
    let sql_console = false;

    Ok((
        [(CACHE_CONTROL, "no-cache")],
        Json(InstanceResponse {
            name: settings.name,
            instance_url: settings.instance_url,
            uri_prefix: settings.uri_prefix,
            front_page_text: settings.front_page_text,
            setup_required: settings.setup_required,
            policies: InstancePolicies {
                allow_public_signup: settings.allow_public_signup,
                require_login: settings.require_login,
            },
            capabilities: InstanceCapabilities {
                browser_sessions: true,
                legacy_api: true,
                structured_search: true,
                sequence_search: true,
                data_lab,
                sql_console,
            },
        }),
    ))
}
