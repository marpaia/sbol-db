//! Typed instance configuration for the SBOL DB administrator UI.

use axum::body::Bytes;
use axum::extract::State;
use axum::{Extension, Json};
use sbol_db_app::{AdminAuditOutcome, ConfigError};
use sbol_db_core::IriString;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::super::auth::Identity;
use super::super::error::V2Error;
use super::super::util::parse_json;
use crate::error::ApiError;
use crate::instance::{legacy_theme, public_settings, THEME_KEY};
use crate::AppState;

#[derive(Debug, Serialize)]
pub(super) struct InstanceAdminResponse {
    name: String,
    instance_url: String,
    uri_prefix: String,
    front_page_text: String,
    allow_public_signup: bool,
    require_login: bool,
    setup_required: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct InstancePatch {
    name: Option<String>,
    instance_url: Option<String>,
    uri_prefix: Option<String>,
    front_page_text: Option<String>,
    allow_public_signup: Option<bool>,
    require_login: Option<bool>,
}

pub(super) async fn get(
    State(state): State<AppState>,
) -> Result<Json<InstanceAdminResponse>, V2Error> {
    Ok(Json(response(&state).await?))
}

pub(super) async fn patch(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    body: Bytes,
) -> Result<Json<InstanceAdminResponse>, V2Error> {
    let request: InstancePatch = parse_json(&body)?;
    let actor = identity
        .0
        .as_ref()
        .map(|user| user.username.as_str())
        .unwrap_or("unknown");
    let audit = state.app.admin_audit_service();
    audit
        .record(
            "instance.update",
            actor,
            THEME_KEY,
            AdminAuditOutcome::Attempted,
            None,
        )
        .await?;

    let mut theme = legacy_theme(&state).await?;
    let object = theme.as_object_mut().ok_or_else(|| {
        V2Error::from(ApiError::BadRequest(
            "stored instance configuration is not an object".to_owned(),
        ))
    })?;
    if let Some(name) = request.name {
        object.insert(
            "instanceName".to_owned(),
            Value::String(required(name, "name")?),
        );
    }
    if let Some(instance_url) = request.instance_url {
        validate_instance_url(&instance_url)?;
        object.insert("instanceUrl".to_owned(), Value::String(instance_url));
    }
    if let Some(uri_prefix) = request.uri_prefix {
        let uri_prefix = required(uri_prefix, "uri_prefix")?;
        IriString::new(uri_prefix.clone())
            .map_err(|error| ApiError::BadRequest(error.to_string()))?;
        object.insert("uriPrefix".to_owned(), Value::String(uri_prefix));
    }
    if let Some(front_page_text) = request.front_page_text {
        object.insert("frontPageText".to_owned(), Value::String(front_page_text));
    }
    if let Some(allow) = request.allow_public_signup {
        object.insert("allowPublicSignup".to_owned(), Value::Bool(allow));
    }
    if let Some(require) = request.require_login {
        object.insert("requireLogin".to_owned(), Value::Bool(require));
    }
    state
        .app
        .config_service()
        .set(true, THEME_KEY, &theme)
        .await
        .map_err(config_error)?;
    audit
        .record(
            "instance.update",
            actor,
            THEME_KEY,
            AdminAuditOutcome::Succeeded,
            None,
        )
        .await?;
    Ok(Json(response(&state).await?))
}

async fn response(state: &AppState) -> Result<InstanceAdminResponse, V2Error> {
    let settings = public_settings(state).await?;
    Ok(InstanceAdminResponse {
        name: settings.name,
        instance_url: settings.instance_url,
        uri_prefix: settings.uri_prefix,
        front_page_text: settings.front_page_text,
        allow_public_signup: settings.allow_public_signup,
        require_login: settings.require_login,
        setup_required: settings.setup_required,
    })
}

fn required(value: String, field: &str) -> Result<String, V2Error> {
    let value = value.trim();
    if value.is_empty() {
        Err(ApiError::BadRequest(format!("{field} must not be empty")).into())
    } else {
        Ok(value.to_owned())
    }
}

fn validate_instance_url(value: &str) -> Result<(), V2Error> {
    if value.is_empty() {
        return Ok(());
    }
    let url = url::Url::parse(value)
        .map_err(|_| ApiError::BadRequest("instance_url must be an absolute URL".to_owned()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ApiError::BadRequest(
            "instance_url must use http or https and include a host".to_owned(),
        )
        .into());
    }
    Ok(())
}

fn config_error(error: ConfigError) -> V2Error {
    match error {
        ConfigError::NotAuthorized => {
            ApiError::Forbidden("administrator access is required".to_owned()).into()
        }
        ConfigError::Domain(error) => error.into(),
    }
}
