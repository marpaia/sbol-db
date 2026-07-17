//! External plugin configuration and proxying, plus the temp-file expose and
//! async stream handoffs plugins use.
//!
//! Classic SynBioHub treats plugins as external HTTP services registered by an
//! administrator under five categories (`rendering`, `download`, `submit`,
//! `curation`, `authorization`), each a `{name, url}` list stored in the mutable
//! `config.local.json`. The `/callPlugin` endpoint proxies a request to the
//! configured plugin URL (`lib/plugins/pluginEndpoints.js`); `/expose/:id` hands
//! a plugin a temporary artifact under a time-limited id (`lib/api/expose.js`);
//! `/stream/:id` is the async long-run handoff that answers `503 Retry-After`
//! while a plugin's work is in flight and the payload once it resolves
//! (`lib/api/stream.js`).
//!
//! [`PluginService`] is the durable, app-layer replacement: the plugin lists
//! live under one [`ConfigStore`] key, admin mutations re-check the caller is an
//! administrator (matching [`ConfigService`](crate::ConfigService)), and outbound
//! HTTP is abstracted behind [`PluginClient`] so the proxy is testable against a
//! stub with no network. The production [`HttpPluginClient`] runs every plugin
//! URL through the same SSRF guard the federation and remote-import paths use.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use sbol_db_core::DomainError;
use sbol_db_storage::ConfigStore;
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::federation::validate_public_https_url;

/// The config key holding the plugin lists, one array per category.
pub const PLUGINS_KEY: &str = "plugins";
/// This instance's public base URL, used to build the export URLs a `run`
/// request hands a rendering/download plugin.
const INSTANCE_URL_KEY: &str = "instanceUrl";

/// The five plugin categories classic SynBioHub recognizes.
pub const PLUGIN_CATEGORIES: [&str; 5] = [
    "rendering",
    "download",
    "submit",
    "curation",
    "authorization",
];

/// The lifetime of an exposed artifact before it is swept, matching classic's
/// `exposeLifetime` of ten minutes.
const EXPOSE_TTL: Duration = Duration::from_secs(10 * 60);

/// The default per-endpoint outbound timeout for a plugin call.
const PLUGIN_TIMEOUT: Duration = Duration::from_secs(60);
/// The connect timeout for a plugin call.
const PLUGIN_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// A plugin operation's failure mode, carrying the HTTP status the adapter maps
/// it to.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    /// An admin-only mutation was attempted by a non-administrator (`403`).
    #[error("administrator privileges are required")]
    NotAuthorized,
    /// A required field was missing or invalid (`400`).
    #[error("{0}")]
    MissingField(String),
    /// The named plugin or the requested endpoint does not exist (`404`).
    #[error("{0}")]
    NotFound(String),
    /// Contacting the plugin failed (`500`, matching classic's plugin errors).
    #[error("{0}")]
    Contact(String),
    #[error(transparent)]
    Domain(#[from] DomainError),
}

/// One proxied plugin response: the status, the body bytes, and the two headers
/// classic forwards for a download plugin.
#[derive(Clone, Debug, Default)]
pub struct PluginResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
    pub content_disposition: Option<String>,
}

impl PluginResponse {
    /// A `text/plain` response with the given status and body, the shape the
    /// `message` short-circuit returns.
    pub fn text(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into().into_bytes(),
            content_type: Some("text/plain".to_owned()),
            content_disposition: None,
        }
    }
}

/// The parsed `/callPlugin` request.
#[derive(Clone, Debug, Default)]
pub struct CallPluginRequest {
    /// The plugin name to resolve within the category.
    pub name: String,
    /// One of `status`, `evaluate`, `run` (or `message`, handled by category).
    pub endpoint: String,
    /// One of the five plugin categories, or `message`.
    pub category: String,
    /// The opaque payload forwarded to `evaluate` / `run`.
    pub data: Value,
    /// An optional URI prefix overriding this instance's base URL when building
    /// the export URLs handed to a rendering/download plugin.
    pub prefix: Option<String>,
    /// The message body echoed back when `category == "message"`.
    pub message: Option<String>,
}

/// Outbound plugin HTTP, abstracted so the proxy is testable against a stub with
/// no network.
#[async_trait]
pub trait PluginClient: Send + Sync {
    /// GET `url`, returning the proxied response.
    async fn get(&self, url: &str) -> Result<PluginResponse, PluginError>;
    /// POST `body` as JSON to `url`, returning the proxied response.
    async fn post(&self, url: &str, body: &Value) -> Result<PluginResponse, PluginError>;
}

/// The production [`PluginClient`]: `reqwest` with the SSRF guard on every URL,
/// no redirects, and bounded timeouts.
pub struct HttpPluginClient {
    client: reqwest::Client,
}

impl Default for HttpPluginClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpPluginClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(PLUGIN_CONNECT_TIMEOUT)
            .timeout(PLUGIN_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("sbol-db/", env!("CARGO_PKG_VERSION"), " plugin"))
            .build()
            .expect("plugin HTTP client construction cannot fail");
        Self { client }
    }
}

/// Turn a `reqwest` response into a [`PluginResponse`], preserving the status
/// and the `Content-Type` / `Content-Disposition` headers.
async fn into_plugin_response(response: reqwest::Response) -> Result<PluginResponse, PluginError> {
    let status = response.status().as_u16();
    let content_type = header_value(&response, reqwest::header::CONTENT_TYPE);
    let content_disposition = header_value(&response, reqwest::header::CONTENT_DISPOSITION);
    let body = response
        .bytes()
        .await
        .map_err(|e| PluginError::Contact(format!("reading plugin response failed: {e}")))?
        .to_vec();
    Ok(PluginResponse {
        status,
        body,
        content_type,
        content_disposition,
    })
}

fn header_value(response: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

fn contact_err(name: &str, e: impl std::fmt::Display) -> PluginError {
    PluginError::Contact(format!(
        "The plugin {name} endpoint is not responding. Check that the plugin is active and running. {e}"
    ))
}

#[async_trait]
impl PluginClient for HttpPluginClient {
    async fn get(&self, url: &str) -> Result<PluginResponse, PluginError> {
        let url =
            validate_public_https_url(url).map_err(|e| PluginError::Contact(e.to_string()))?;
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| contact_err("", e))?;
        into_plugin_response(response).await
    }

    async fn post(&self, url: &str, body: &Value) -> Result<PluginResponse, PluginError> {
        let url =
            validate_public_https_url(url).map_err(|e| PluginError::Contact(e.to_string()))?;
        let response = self
            .client
            .post(url)
            .json(body)
            .send()
            .await
            .map_err(|e| contact_err("", e))?;
        into_plugin_response(response).await
    }
}

/// Plugin configuration and proxying over a durable [`ConfigStore`], with
/// outbound HTTP behind a [`PluginClient`].
#[derive(Clone)]
pub struct PluginService {
    config: Arc<dyn ConfigStore>,
    client: Arc<dyn PluginClient>,
}

impl PluginService {
    /// Build the service over a config store and a plugin client.
    pub fn new(config: Arc<dyn ConfigStore>, client: Arc<dyn PluginClient>) -> Self {
        Self { config, client }
    }

    /// The configured plugins, one array per category. A category with no
    /// registered plugins reads back as an empty array, so a fresh instance
    /// still returns the full five-category shape.
    pub async fn plugins(&self) -> Result<Map<String, Value>, DomainError> {
        self.plugins_map().await
    }

    /// Save (append or replace) a plugin in a category. `id == "New"` appends;
    /// any other `id` is a one-based index into the category list that is
    /// replaced. Admin-gated.
    pub async fn save_plugin(
        &self,
        is_admin: bool,
        category: &str,
        id: &str,
        name: &str,
        url: &str,
    ) -> Result<String, PluginError> {
        if !is_admin {
            return Err(PluginError::NotAuthorized);
        }
        Self::validate_category(category)?;
        if name.trim().is_empty() {
            return Err(PluginError::MissingField(
                "a valid plugin name is required".to_owned(),
            ));
        }
        if id.trim().is_empty() {
            return Err(PluginError::MissingField(
                "a valid plugin id is required".to_owned(),
            ));
        }
        if url.trim().is_empty() {
            return Err(PluginError::MissingField(
                "a valid plugin URL is required".to_owned(),
            ));
        }
        // Every plugin URL classic stores ends in a slash so the endpoint suffix
        // (`status` / `evaluate` / `run`) appends cleanly.
        let url = if url.ends_with('/') {
            url.to_owned()
        } else {
            format!("{url}/")
        };

        let mut map = self.plugins_map().await?;
        let list = category_list_mut(&mut map, category);
        let entry = serde_json::json!({ "name": name, "url": url });
        if id == "New" {
            list.push(entry);
        } else {
            let index = parse_index(id)?;
            let slot = list.get_mut(index).ok_or_else(|| {
                PluginError::NotFound(format!("plugin not found at index {id} in {category}"))
            })?;
            *slot = entry;
        }
        self.config.set(PLUGINS_KEY, &Value::Object(map)).await?;
        Ok(name.to_owned())
    }

    /// Remove a plugin from a category by its one-based index. Admin-gated;
    /// `404` if the index is out of range.
    pub async fn delete_plugin(
        &self,
        is_admin: bool,
        category: &str,
        id: &str,
    ) -> Result<(), PluginError> {
        if !is_admin {
            return Err(PluginError::NotAuthorized);
        }
        Self::validate_category(category)?;
        if id.trim().is_empty() {
            return Err(PluginError::MissingField(
                "a valid plugin id is required".to_owned(),
            ));
        }
        let index = parse_index(id)?;
        let mut map = self.plugins_map().await?;
        let list = category_list_mut(&mut map, category);
        if index >= list.len() {
            return Err(PluginError::NotFound(format!(
                "plugin not found at index {id} in {category}"
            )));
        }
        list.remove(index);
        self.config.set(PLUGINS_KEY, &Value::Object(map)).await?;
        Ok(())
    }

    /// Proxy a `/callPlugin` request to the configured plugin. Mirrors classic
    /// `pluginEndpoints.js`: the `message` category short-circuits, an unknown
    /// plugin or endpoint is `404`, and `status` / `evaluate` / `run` proxy to
    /// the plugin's matching endpoint. Not admin-gated.
    pub async fn call_plugin(
        &self,
        req: &CallPluginRequest,
    ) -> Result<PluginResponse, PluginError> {
        if req.category == "message" {
            let message = req.message.clone().unwrap_or_default();
            return Ok(PluginResponse::text(
                200,
                format!("Message sent: {message}"),
            ));
        }
        let base = self
            .find_plugin(&req.category, &req.name)
            .await?
            .ok_or_else(|| {
                PluginError::NotFound(format!(
                    "The plugin {} was not found or there is no url associated with this name.",
                    req.name
                ))
            })?;
        match req.endpoint.as_str() {
            "status" => self
                .client
                .get(&format!("{base}status"))
                .await
                .map_err(|e| rename_contact(&req.name, e)),
            "evaluate" => self
                .client
                .post(&format!("{base}evaluate"), &req.data)
                .await
                .map_err(|e| rename_contact(&req.name, e)),
            "run" => {
                let body = self.build_run_body(req).await?;
                self.client
                    .post(&format!("{base}run"), &body)
                    .await
                    .map_err(|e| rename_contact(&req.name, e))
            }
            other => Err(PluginError::NotFound(format!(
                "This plugin endpoint {other} is not known. Instead try status, evaluate, or run."
            ))),
        }
    }

    /// Resolve a plugin's base URL by name within a category, or `None` when no
    /// plugin in that category carries the name.
    async fn find_plugin(&self, category: &str, name: &str) -> Result<Option<String>, DomainError> {
        let map = self.plugins_map().await?;
        let list = map.get(category).and_then(Value::as_array);
        Ok(list.and_then(|list| {
            list.iter()
                .find(|p| p.get("name").and_then(Value::as_str) == Some(name))
                .and_then(|p| p.get("url").and_then(Value::as_str))
                .map(str::to_owned)
        }))
    }

    /// Build the JSON body a `run` request forwards. A rendering/download
    /// category augments the caller's data with the object's export URLs
    /// (`complete_sbol` / `shallow_sbol` / `genbank`), matching classic's
    /// `getPublicDataFromURI`; every other category forwards the data verbatim.
    async fn build_run_body(&self, req: &CallPluginRequest) -> Result<Value, PluginError> {
        if req.category != "rendering" && req.category != "download" {
            return Ok(req.data.clone());
        }
        let Value::Object(data) = &req.data else {
            return Ok(req.data.clone());
        };
        let suffix = data
            .get("uriSuffix")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let base = match &req.prefix {
            Some(prefix) if !prefix.is_empty() => prefix.clone(),
            _ => self.string_setting(INSTANCE_URL_KEY).await?,
        };
        let uri = format!("{base}{suffix}");
        let mut augmented = data.clone();
        augmented.insert(
            "complete_sbol".to_owned(),
            Value::String(format!("{uri}/sbol")),
        );
        augmented.insert(
            "shallow_sbol".to_owned(),
            Value::String(format!("{uri}/sbolnr")),
        );
        augmented.insert("genbank".to_owned(), Value::String(format!("{uri}/gb")));
        if let Some(top) = data.get("top") {
            augmented.insert("top_level".to_owned(), top.clone());
        }
        Ok(Value::Object(augmented))
    }

    async fn string_setting(&self, key: &str) -> Result<String, DomainError> {
        Ok(self
            .config
            .get(key)
            .await?
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default())
    }

    /// The stored plugins map, seeded with an empty array for every category so
    /// callers always see the full five-category shape.
    async fn plugins_map(&self) -> Result<Map<String, Value>, DomainError> {
        let mut map = match self.config.get(PLUGINS_KEY).await? {
            Some(Value::Object(map)) => map,
            _ => Map::new(),
        };
        for category in PLUGIN_CATEGORIES {
            map.entry(category.to_owned())
                .or_insert_with(|| Value::Array(Vec::new()));
        }
        Ok(map)
    }

    fn validate_category(category: &str) -> Result<(), PluginError> {
        if PLUGIN_CATEGORIES.contains(&category) {
            Ok(())
        } else {
            Err(PluginError::MissingField(
                "a valid category (rendering, download, submit, curation, authorization) is required"
                    .to_owned(),
            ))
        }
    }
}

/// Rewrite a client [`PluginError::Contact`] with the plugin name filled in, so
/// the proxied error names the offending plugin like classic does.
fn rename_contact(name: &str, err: PluginError) -> PluginError {
    match err {
        PluginError::Contact(_) => contact_err(name, "plugin request failed"),
        other => other,
    }
}

/// The one-based plugin index parsed to a zero-based offset.
fn parse_index(id: &str) -> Result<usize, PluginError> {
    let n: usize = id
        .parse()
        .map_err(|_| PluginError::MissingField(format!("plugin id must be a number, got {id}")))?;
    n.checked_sub(1)
        .ok_or_else(|| PluginError::MissingField("plugin id must be one or greater".to_owned()))
}

/// Borrow (creating if absent) a category's plugin array as a mutable `Vec`.
fn category_list_mut<'a>(map: &'a mut Map<String, Value>, category: &str) -> &'a mut Vec<Value> {
    map.entry(category.to_owned())
        .or_insert_with(|| Value::Array(Vec::new()));
    map.get_mut(category)
        .and_then(Value::as_array_mut)
        .expect("category entry was just inserted as an array")
}

/// A time-limited registry of exposed temp-file artifacts, the durable-free
/// counterpart to classic `lib/api/expose.js`. An id maps to a path for
/// [`EXPOSE_TTL`]; a read past the lifetime drops the entry and reports it gone.
#[derive(Default)]
pub struct ExposeRegistry {
    entries: Mutex<HashMap<Uuid, (PathBuf, Instant)>>,
}

impl ExposeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `path` and return its id.
    pub fn create(&self, path: PathBuf) -> Uuid {
        let id = Uuid::new_v4();
        self.entries
            .lock()
            .unwrap()
            .insert(id, (path, Instant::now()));
        id
    }

    /// The path registered under `id`, or `None` when it is unknown or its
    /// lifetime has elapsed. An expired entry is dropped on access.
    pub fn get(&self, id: Uuid) -> Option<PathBuf> {
        let mut entries = self.entries.lock().unwrap();
        match entries.get(&id) {
            Some((path, created)) if created.elapsed() < EXPOSE_TTL => Some(path.clone()),
            Some(_) => {
                entries.remove(&id);
                None
            }
            None => None,
        }
    }

    /// Register `path` with an explicit creation instant, so a test can insert
    /// an already-expired artifact without waiting out the lifetime.
    #[doc(hidden)]
    pub fn create_at(&self, path: PathBuf, created: Instant) -> Uuid {
        let id = Uuid::new_v4();
        self.entries.lock().unwrap().insert(id, (path, created));
        id
    }
}

/// The resolved payload of a stream, delivered once the backing work finishes.
#[derive(Clone, Debug, Default)]
pub struct StreamOutcome {
    pub body: Vec<u8>,
    pub content_type: Option<String>,
}

/// What a `/stream/:id` read should return, mapped by the adapter to a status.
#[derive(Clone, Debug)]
pub enum StreamServe {
    /// The work is still in flight: answer `503` with `Retry-After: 1`.
    Pending,
    /// The work finished: return the payload with `200`.
    Ready(StreamOutcome),
    /// The stream was explicitly cleared: `410 Gone`.
    Cleared,
    /// No such stream (or it failed): `404`.
    Gone,
}

enum StreamSlot {
    Pending,
    Ready(StreamOutcome),
    Cleared,
    Failed,
}

/// The async long-run handoff registry, the counterpart to classic
/// `lib/api/stream.js`. A stream starts [`create`](Self::create)d as pending;
/// while the backing work runs a read answers [`StreamServe::Pending`] (the
/// `503 Retry-After` poll), and once the caller [`resolve`](Self::resolve)s it
/// the payload is served. A `DELETE` [`clear`](Self::clear)s it. The registry is
/// a passive state store: the caller (which owns a runtime) drives the work and
/// reports the result, keeping this crate runtime-agnostic.
#[derive(Default)]
pub struct StreamRegistry {
    slots: Mutex<HashMap<Uuid, StreamSlot>>,
}

impl StreamRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new pending stream and return its id.
    pub fn create(&self) -> Uuid {
        let id = Uuid::new_v4();
        self.slots.lock().unwrap().insert(id, StreamSlot::Pending);
        id
    }

    /// Deliver a stream's payload. A stream cleared before completion stays
    /// cleared, matching classic's race handling.
    pub fn resolve(&self, id: Uuid, outcome: StreamOutcome) {
        let mut slots = self.slots.lock().unwrap();
        if let Some(slot) = slots.get_mut(&id) {
            if !matches!(slot, StreamSlot::Cleared) {
                *slot = StreamSlot::Ready(outcome);
            }
        }
    }

    /// Mark a stream failed, so a later read reports it gone.
    pub fn fail(&self, id: Uuid) {
        let mut slots = self.slots.lock().unwrap();
        if let Some(slot) = slots.get_mut(&id) {
            if !matches!(slot, StreamSlot::Cleared) {
                *slot = StreamSlot::Failed;
            }
        }
    }

    /// Read the current state of `id` for a `GET`.
    pub fn serve(&self, id: Uuid) -> StreamServe {
        match self.slots.lock().unwrap().get(&id) {
            Some(StreamSlot::Pending) => StreamServe::Pending,
            Some(StreamSlot::Ready(outcome)) => StreamServe::Ready(outcome.clone()),
            Some(StreamSlot::Cleared) => StreamServe::Cleared,
            Some(StreamSlot::Failed) | None => StreamServe::Gone,
        }
    }

    /// Clear `id`, so a later read reports it cleared (`410`).
    pub fn clear(&self, id: Uuid) {
        self.slots.lock().unwrap().insert(id, StreamSlot::Cleared);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;
    use crate::memory::InMemoryConfigStore;

    /// A stub plugin client recording the last request and returning a canned
    /// response, so the proxy is exercised with no network.
    #[derive(Default)]
    struct StubClient {
        last: Mutex<Option<(String, Option<Value>)>>,
    }

    #[async_trait]
    impl PluginClient for StubClient {
        async fn get(&self, url: &str) -> Result<PluginResponse, PluginError> {
            *self.last.lock().unwrap() = Some((url.to_owned(), None));
            Ok(PluginResponse::text(200, "stub-status-ok"))
        }
        async fn post(&self, url: &str, body: &Value) -> Result<PluginResponse, PluginError> {
            *self.last.lock().unwrap() = Some((url.to_owned(), Some(body.clone())));
            Ok(PluginResponse::text(200, "stub-run-ok"))
        }
    }

    fn service(client: Arc<StubClient>) -> PluginService {
        PluginService::new(Arc::new(InMemoryConfigStore::new()), client)
    }

    #[tokio::test]
    async fn save_list_delete_roundtrip() {
        let svc = service(Arc::new(StubClient::default()));

        // A non-admin cannot save.
        assert!(matches!(
            svc.save_plugin(false, "rendering", "New", "viz", "https://viz.example.org")
                .await,
            Err(PluginError::NotAuthorized)
        ));
        // An unknown category is rejected.
        assert!(matches!(
            svc.save_plugin(true, "bogus", "New", "viz", "https://viz.example.org")
                .await,
            Err(PluginError::MissingField(_))
        ));

        // A valid save appends and normalizes the URL to a trailing slash.
        svc.save_plugin(true, "rendering", "New", "viz", "https://viz.example.org")
            .await
            .expect("save");
        let plugins = svc.plugins().await.expect("plugins");
        let rendering = plugins["rendering"].as_array().unwrap();
        assert_eq!(rendering.len(), 1);
        assert_eq!(rendering[0]["name"], "viz");
        assert_eq!(rendering[0]["url"], "https://viz.example.org/");
        // Every category is present even when empty.
        assert!(plugins["download"].as_array().unwrap().is_empty());

        // Delete by one-based index; a second delete is a 404.
        svc.delete_plugin(true, "rendering", "1")
            .await
            .expect("delete");
        assert!(matches!(
            svc.delete_plugin(true, "rendering", "1").await,
            Err(PluginError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn call_plugin_dispatches_by_endpoint() {
        let client = Arc::new(StubClient::default());
        let svc = service(client.clone());
        svc.save_plugin(
            true,
            "curation",
            "New",
            "curate",
            "https://curate.example.org",
        )
        .await
        .expect("save");

        // message short-circuits without touching the client.
        let res = svc
            .call_plugin(&CallPluginRequest {
                category: "message".to_owned(),
                message: Some("hi".to_owned()),
                ..CallPluginRequest::default()
            })
            .await
            .expect("message");
        assert_eq!(res.status, 200);
        assert_eq!(res.body, b"Message sent: hi");

        // status proxies a GET to `{url}status`.
        let res = svc
            .call_plugin(&CallPluginRequest {
                name: "curate".to_owned(),
                endpoint: "status".to_owned(),
                category: "curation".to_owned(),
                ..CallPluginRequest::default()
            })
            .await
            .expect("status");
        assert_eq!(res.body, b"stub-status-ok");
        assert_eq!(
            client.last.lock().unwrap().clone().unwrap().0,
            "https://curate.example.org/status"
        );

        // run proxies a POST to `{url}run`.
        let res = svc
            .call_plugin(&CallPluginRequest {
                name: "curate".to_owned(),
                endpoint: "run".to_owned(),
                category: "curation".to_owned(),
                data: json!({ "k": "v" }),
                ..CallPluginRequest::default()
            })
            .await
            .expect("run");
        assert_eq!(res.body, b"stub-run-ok");
        let (url, body) = client.last.lock().unwrap().clone().unwrap();
        assert_eq!(url, "https://curate.example.org/run");
        assert_eq!(body.unwrap(), json!({ "k": "v" }));

        // An unknown plugin name is a 404.
        assert!(matches!(
            svc.call_plugin(&CallPluginRequest {
                name: "missing".to_owned(),
                endpoint: "status".to_owned(),
                category: "curation".to_owned(),
                ..CallPluginRequest::default()
            })
            .await,
            Err(PluginError::NotFound(_))
        ));
    }

    #[test]
    fn expose_serves_until_expiry() {
        let registry = ExposeRegistry::new();
        let id = registry.create(PathBuf::from("/tmp/artifact.xml"));
        assert_eq!(
            registry.get(id).as_deref(),
            Some(std::path::Path::new("/tmp/artifact.xml"))
        );

        // An entry created past its lifetime reads back as gone.
        let stale = registry.create_at(
            PathBuf::from("/tmp/old.xml"),
            Instant::now() - EXPOSE_TTL - Duration::from_secs(1),
        );
        assert!(registry.get(stale).is_none());
    }

    #[test]
    fn stream_polls_then_resolves() {
        let registry = StreamRegistry::new();

        // A fresh stream is pending: the read that drives the 503 poll.
        let id = registry.create();
        assert!(matches!(registry.serve(id), StreamServe::Pending));

        // Once resolved the payload is served.
        registry.resolve(
            id,
            StreamOutcome {
                body: b"payload".to_vec(),
                content_type: Some("text/plain".to_owned()),
            },
        );
        let StreamServe::Ready(outcome) = registry.serve(id) else {
            panic!("stream should be ready");
        };
        assert_eq!(outcome.body, b"payload");

        // A clear moves it to the cleared state, and a resolve after a clear is
        // ignored.
        registry.clear(id);
        registry.resolve(id, StreamOutcome::default());
        assert!(matches!(registry.serve(id), StreamServe::Cleared));

        // A failed stream and an unknown id both read as gone.
        let failed = registry.create();
        registry.fail(failed);
        assert!(matches!(registry.serve(failed), StreamServe::Gone));
        assert!(matches!(registry.serve(Uuid::new_v4()), StreamServe::Gone));
    }
}
