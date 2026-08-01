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
    #[serde(skip_serializing_if = "Option::is_none")]
    machine_access: Option<MachineAccess>,
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
    profile_management: bool,
    password_change: bool,
    password_reset: bool,
    collaboration: bool,
    data_lab: bool,
    sql_console: bool,
}

/// Absolute endpoints a machine client can safely discover from a trusted
/// deployment origin. Optional services are omitted until they are mounted.
#[derive(Debug, Serialize)]
struct MachineAccess {
    api_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    mcp_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    authorization_issuer: Option<String>,
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
    let machine_access = state
        .config
        .public_origin
        .as_ref()
        .map(|origin| MachineAccess {
            api_url: format!("{origin}/api/v2"),
            mcp_url: state.config.mcp_enabled.then(|| format!("{origin}/mcp")),
            // This becomes Some only when the corresponding OAuth/OIDC
            // provider is actually mounted. Advertising it early would make
            // capability discovery indistinguishable from forward-looking copy.
            authorization_issuer: None,
        });

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
                profile_management: true,
                password_change: true,
                // No delivery worker is installed yet. Keeping this false
                // prevents the native UI from exposing a reset flow that could
                // mint an undeliverable secret.
                password_reset: false,
                collaboration: true,
                data_lab,
                sql_console,
            },
            machine_access,
        }),
    ))
}
