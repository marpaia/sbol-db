//! Federation, remote, and plugin configuration with secret-safe reads.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use sbol_db_app::{AdminAuditOutcome, FederationError, PluginError};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::super::auth::Identity;
use super::super::error::V2Error;
use super::super::util::parse_json;
use super::confirmation;
use crate::error::ApiError;
use crate::AppState;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FederationRequest {
    administrator_email: String,
    url: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RegistryRequest {
    uri: String,
    url: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PluginRequest {
    category: String,
    #[serde(default = "new_plugin_id")]
    id: String,
    name: String,
    url: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Confirmation {
    confirmation: String,
}

fn new_plugin_id() -> String {
    "New".to_owned()
}

pub(super) async fn get(State(state): State<AppState>) -> Result<Json<Value>, V2Error> {
    let federation = state.app.federation();
    let registries: Vec<Value> = federation
        .registries()
        .await?
        .into_iter()
        .map(|(uri, url)| json!({ "uri": uri, "url": url }))
        .collect();
    let mut remotes = Value::Object(federation.remotes().await?);
    redact_secrets(&mut remotes, None);
    Ok(Json(json!({
        "federation": {
            "registered": federation.is_registered().await?,
            "url": federation.web_of_registries_url().await?,
        },
        "registries": registries,
        "remotes": remotes,
        "plugins": Value::Object(state.app.plugins().plugins().await?),
    })))
}

pub(super) async fn federate(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    body: Bytes,
) -> Result<Json<Value>, V2Error> {
    let request: FederationRequest = parse_json(&body)?;
    let actor = actor(&identity);
    let audit = state.app.admin_audit_service();
    audit
        .record(
            "federation.join",
            actor,
            &request.url,
            AdminAuditOutcome::Attempted,
            None,
        )
        .await?;
    state
        .app
        .federation()
        .federate(true, &request.administrator_email, &request.url)
        .await
        .map_err(federation_error)?;
    audit
        .record(
            "federation.join",
            actor,
            &request.url,
            AdminAuditOutcome::Succeeded,
            None,
        )
        .await?;
    Ok(Json(json!({ "status": "joined" })))
}

pub(super) async fn sync(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<Value>, V2Error> {
    let actor = actor(&identity);
    let audit = state.app.admin_audit_service();
    audit
        .record(
            "federation.sync",
            actor,
            "web-of-registries",
            AdminAuditOutcome::Attempted,
            None,
        )
        .await?;
    let count = state
        .app
        .federation()
        .retrieve()
        .await
        .map_err(federation_error)?;
    audit
        .record(
            "federation.sync",
            actor,
            "web-of-registries",
            AdminAuditOutcome::Succeeded,
            Some(&format!("{count} registries applied")),
        )
        .await?;
    Ok(Json(json!({ "status": "synchronized", "count": count })))
}

pub(super) async fn save_registry(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    body: Bytes,
) -> Result<Json<Value>, V2Error> {
    let request: RegistryRequest = parse_json(&body)?;
    state
        .app
        .federation()
        .save_registry(true, &request.uri, &request.url)
        .await
        .map_err(federation_error)?;
    record_success(&state, &identity, "registry.save", &request.uri).await?;
    Ok(Json(json!({ "status": "saved", "uri": request.uri })))
}

pub(super) async fn delete_registry(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(uri): Path<String>,
    body: Bytes,
) -> Result<StatusCode, V2Error> {
    let request: Confirmation = parse_json(&body)?;
    confirmation(&request.confirmation, &format!("DELETE REGISTRY {uri}"))?;
    record_attempt(&state, &identity, "registry.delete", &uri).await?;
    state
        .app
        .federation()
        .delete_registry(true, &uri)
        .await
        .map_err(federation_error)?;
    record_success(&state, &identity, "registry.delete", &uri).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Save a remote object verbatim, but only return its id. Reads pass through
/// recursive secret redaction so credentials never re-enter the browser.
pub(super) async fn save_remote(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    body: Bytes,
) -> Result<Json<Value>, V2Error> {
    let remote: Value = parse_json(&body)?;
    let id = state
        .app
        .federation()
        .save_remote(true, remote)
        .await
        .map_err(federation_error)?;
    record_success(&state, &identity, "remote.save", &id).await?;
    Ok(Json(json!({ "status": "saved", "id": id })))
}

pub(super) async fn delete_remote(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<StatusCode, V2Error> {
    let request: Confirmation = parse_json(&body)?;
    confirmation(&request.confirmation, &format!("DELETE REMOTE {id}"))?;
    record_attempt(&state, &identity, "remote.delete", &id).await?;
    state
        .app
        .federation()
        .delete_remote(true, &id)
        .await
        .map_err(federation_error)?;
    record_success(&state, &identity, "remote.delete", &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn save_plugin(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    body: Bytes,
) -> Result<Json<Value>, V2Error> {
    let request: PluginRequest = parse_json(&body)?;
    let name = state
        .app
        .plugins()
        .save_plugin(
            true,
            &request.category,
            &request.id,
            &request.name,
            &request.url,
        )
        .await
        .map_err(plugin_error)?;
    let target = format!("{}/{}", request.category, request.id);
    record_success(&state, &identity, "plugin.save", &target).await?;
    Ok(Json(json!({ "status": "saved", "name": name })))
}

pub(super) async fn delete_plugin(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path((category, id)): Path<(String, String)>,
    body: Bytes,
) -> Result<StatusCode, V2Error> {
    let request: Confirmation = parse_json(&body)?;
    let target = format!("{category}/{id}");
    confirmation(&request.confirmation, &format!("DELETE PLUGIN {target}"))?;
    record_attempt(&state, &identity, "plugin.delete", &target).await?;
    state
        .app
        .plugins()
        .delete_plugin(true, &category, &id)
        .await
        .map_err(plugin_error)?;
    record_success(&state, &identity, "plugin.delete", &target).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn record_attempt(
    state: &AppState,
    identity: &Identity,
    action: &str,
    target: &str,
) -> Result<(), V2Error> {
    state
        .app
        .admin_audit_service()
        .record(
            action,
            actor(identity),
            target,
            AdminAuditOutcome::Attempted,
            None,
        )
        .await?;
    Ok(())
}

async fn record_success(
    state: &AppState,
    identity: &Identity,
    action: &str,
    target: &str,
) -> Result<(), V2Error> {
    state
        .app
        .admin_audit_service()
        .record(
            action,
            actor(identity),
            target,
            AdminAuditOutcome::Succeeded,
            None,
        )
        .await?;
    Ok(())
}

fn actor(identity: &Identity) -> &str {
    identity
        .0
        .as_ref()
        .map(|user| user.username.as_str())
        .unwrap_or("unknown")
}

fn federation_error(error: FederationError) -> V2Error {
    match error {
        FederationError::NotAuthorized => {
            ApiError::Forbidden("administrator access is required".to_owned()).into()
        }
        FederationError::MissingField(message) => ApiError::BadRequest(message).into(),
        FederationError::NotFound(message) => ApiError::NotFound(message).into(),
        FederationError::RemoteContact(message) => ApiError::Unavailable(message).into(),
        FederationError::Domain(error) => error.into(),
    }
}

fn plugin_error(error: PluginError) -> V2Error {
    match error {
        PluginError::NotAuthorized => {
            ApiError::Forbidden("administrator access is required".to_owned()).into()
        }
        PluginError::MissingField(message) => ApiError::BadRequest(message).into(),
        PluginError::NotFound(message) => ApiError::NotFound(message).into(),
        PluginError::Contact(message) => ApiError::Unavailable(message).into(),
        PluginError::Domain(error) => error.into(),
    }
}

fn redact_secrets(value: &mut Value, key: Option<&str>) {
    if key.is_some_and(is_secret_key) {
        *value = Value::String("[redacted]".to_owned());
        return;
    }
    match value {
        Value::Object(object) => {
            let keys: Vec<String> = object.keys().cloned().collect();
            for key in keys {
                if let Some(value) = object.get_mut(&key) {
                    redact_secrets(value, Some(&key));
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_secrets(value, key);
            }
        }
        _ => {}
    }
}

fn is_secret_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace(['-', '_'], "");
    normalized.contains("password")
        || normalized.contains("secret")
        || normalized.contains("token")
        || normalized.contains("apikey")
        || normalized == "key"
        || normalized == "authorization"
}

#[allow(dead_code)]
fn _assert_map_shape(_: Map<String, Value>) {}
