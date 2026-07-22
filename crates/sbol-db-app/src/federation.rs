//! Web of Registries federation and cross-instance object resolution.
//!
//! [`FederationService`] is the durable, app-layer replacement for classic
//! SynBioHub's Web of Registries machinery (`lib/wor.js`,
//! `lib/api/updateWebOfRegistries.js`, `lib/actions/admin/*`). It keeps its
//! whole state in the [`ConfigStore`]: the join credentials from `federate`
//! (`webOfRegistriesUrl`, `webOfRegistriesId`, `webOfRegistriesSecret`), the
//! `uriPrefix -> instanceUrl` map (`webOfRegistries`) that
//! `retrieveFromWebOfRegistries` fills, and the ICE/Benchling `remotes` map.
//!
//! Outbound HTTP is abstracted behind [`WebOfRegistriesClient`] so the service
//! is testable against a stub with no network. The production
//! [`HttpWebOfRegistriesClient`] routes every request through the same
//! SSRF guard the remote-import job uses: HTTPS only, no credentials in the
//! URL, and no private / loopback / link-local host.
//!
//! Admin mutations (`federate`, `save_registry`, `save_remote`, and the
//! deletes) re-check the caller is an administrator, matching the
//! [`ConfigService`](crate::ConfigService) pattern; the sync path
//! (`retrieve`), driven by the public `updateWebOfRegistries` webhook and the
//! `wor_sync` job, is not admin-gated because those callers are authenticated
//! by other means.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Url;
use sbol_db_core::{DomainError, IriString, Triple};
use sbol_db_storage::ConfigStore;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::download::RemoteObjectResolver;

/// The `uriPrefix -> instanceUrl` map filled by federation sync.
pub const WEB_OF_REGISTRIES_KEY: &str = "webOfRegistries";
/// The joined Web of Registries base URL, set by [`FederationService::federate`].
pub const WEB_OF_REGISTRIES_URL_KEY: &str = "webOfRegistriesUrl";
/// The Web of Registries URL shown before an instance has joined one.
pub const DEFAULT_WEB_OF_REGISTRIES_URL: &str = "https://wor.synbiohub.org";
/// This instance's id in the Web of Registries, returned by the join.
pub const WEB_OF_REGISTRIES_ID_KEY: &str = "webOfRegistriesId";
/// The shared secret returned by the join, used to authenticate the
/// `updateWebOfRegistries` webhook.
pub const WEB_OF_REGISTRIES_SECRET_KEY: &str = "webOfRegistriesSecret";
/// The instance administrator's email address.
pub const ADMINISTRATOR_EMAIL_KEY: &str = "administratorEmail";
/// The ICE / Benchling `remotes` map, keyed by remote id.
pub const REMOTES_KEY: &str = "remotes";
/// This instance's public base URL, advertised to the Web of Registries.
pub const INSTANCE_URL_KEY: &str = "instanceUrl";
/// The URI prefix objects are minted under, advertised as this instance's
/// namespace to the Web of Registries.
pub const DATABASE_PREFIX_KEY: &str = "databasePrefix";
/// The instance's human-readable name.
pub const INSTANCE_NAME_KEY: &str = "instanceName";
/// The instance's front-page description text.
pub const FRONT_PAGE_TEXT_KEY: &str = "frontPageText";

/// The `/sbol` suffix appended to a remote object URI to fetch its RDF closure
/// from the hosting instance.
const REMOTE_SBOL_SUFFIX: &str = "/sbol";
/// The maximum time a single outbound federation request may take.
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);
/// The maximum time to establish an outbound federation connection.
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// A federation operation's failure mode, carrying the HTTP status the adapter
/// maps it to.
#[derive(Debug, thiserror::Error)]
pub enum FederationError {
    /// The caller is not an administrator, so an admin-only mutation is refused
    /// (`403`).
    #[error("administrator privileges are required")]
    NotAuthorized,
    /// A required field was missing or empty (`400`).
    #[error("{0}")]
    MissingField(String),
    /// A referenced registry or remote does not exist (`404`).
    #[error("{0}")]
    NotFound(String),
    /// Contacting the Web of Registries or a remote instance failed (`503`).
    #[error("{0}")]
    RemoteContact(String),
    #[error(transparent)]
    Domain(#[from] DomainError),
}

/// One registry instance as advertised by the Web of Registries `/instances`
/// endpoint: the URI prefix it mints under and the URL it serves.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorInstance {
    #[serde(rename = "uriPrefix")]
    pub uri_prefix: String,
    #[serde(rename = "instanceUrl")]
    pub instance_url: String,
}

/// The payload posted to `{worUrl}/instances/new/` to request membership.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JoinPayload {
    #[serde(rename = "instanceUrl")]
    pub instance_url: String,
    #[serde(rename = "uriPrefix")]
    pub uri_prefix: String,
    #[serde(rename = "administratorEmail")]
    pub administrator_email: String,
    #[serde(rename = "updateEndpoint")]
    pub update_endpoint: String,
    pub name: String,
    pub description: String,
}

/// The Web of Registries response to a join request: this instance's assigned
/// id and the secret that authenticates its update webhook.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JoinResponse {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "updateSecret", default)]
    pub update_secret: String,
}

/// Outbound Web of Registries HTTP, abstracted so the service is testable
/// against a stub with no network.
#[async_trait]
pub trait WebOfRegistriesClient: Send + Sync {
    /// POST a join request to `{wor_url}/instances/new/`.
    async fn join(
        &self,
        wor_url: &str,
        payload: &JoinPayload,
    ) -> Result<JoinResponse, FederationError>;

    /// GET the instance list from `{wor_url}/instances`.
    async fn fetch_instances(&self, wor_url: &str) -> Result<Vec<WorInstance>, FederationError>;

    /// GET a remote object's SBOL RDF body from `object_url`.
    async fn fetch_sbol(&self, object_url: &str) -> Result<String, FederationError>;
}

/// The production [`WebOfRegistriesClient`]: `reqwest` with the remote-import
/// SSRF guard on every URL, no redirects, and bounded timeouts.
pub struct HttpWebOfRegistriesClient {
    client: reqwest::Client,
}

impl Default for HttpWebOfRegistriesClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpWebOfRegistriesClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(HTTP_CONNECT_TIMEOUT)
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!(
                "sbol-db/",
                env!("CARGO_PKG_VERSION"),
                " federation"
            ))
            .build()
            .expect("federation HTTP client construction cannot fail");
        Self { client }
    }
}

#[async_trait]
impl WebOfRegistriesClient for HttpWebOfRegistriesClient {
    async fn join(
        &self,
        wor_url: &str,
        payload: &JoinPayload,
    ) -> Result<JoinResponse, FederationError> {
        let url = validate_public_https_url(&format!("{wor_url}/instances/new/"))?;
        let response = self
            .client
            .post(url)
            .json(payload)
            .send()
            .await
            .map_err(contact_err)?
            .error_for_status()
            .map_err(contact_err)?;
        response.json::<JoinResponse>().await.map_err(contact_err)
    }

    async fn fetch_instances(&self, wor_url: &str) -> Result<Vec<WorInstance>, FederationError> {
        let url = validate_public_https_url(&format!("{wor_url}/instances"))?;
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(contact_err)?
            .error_for_status()
            .map_err(contact_err)?;
        response
            .json::<Vec<WorInstance>>()
            .await
            .map_err(contact_err)
    }

    async fn fetch_sbol(&self, object_url: &str) -> Result<String, FederationError> {
        let url = validate_public_https_url(object_url)?;
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(contact_err)?
            .error_for_status()
            .map_err(contact_err)?;
        response.text().await.map_err(contact_err)
    }
}

fn contact_err(e: reqwest::Error) -> FederationError {
    FederationError::RemoteContact(format!("web of registries request failed: {e}"))
}

/// Web of Registries federation over a durable [`ConfigStore`], with outbound
/// HTTP behind a [`WebOfRegistriesClient`].
#[derive(Clone)]
pub struct FederationService {
    config: Arc<dyn ConfigStore>,
    client: Arc<dyn WebOfRegistriesClient>,
}

impl FederationService {
    /// Build the service over a config store and a Web of Registries client.
    pub fn new(config: Arc<dyn ConfigStore>, client: Arc<dyn WebOfRegistriesClient>) -> Self {
        Self { config, client }
    }

    /// Request membership in a Web of Registries. Posts this instance's identity
    /// to `{wor_url}/instances/new/`, then persists the returned id and update
    /// secret alongside the joined URL and the administrator email. Admin-gated.
    pub async fn federate(
        &self,
        is_admin: bool,
        administrator_email: &str,
        wor_url: &str,
    ) -> Result<(), FederationError> {
        if !is_admin {
            return Err(FederationError::NotAuthorized);
        }
        if administrator_email.trim().is_empty() {
            return Err(FederationError::MissingField(
                "a valid administrator email address is required".to_owned(),
            ));
        }
        if wor_url.trim().is_empty() {
            return Err(FederationError::MissingField(
                "a valid Web-of-Registries URL is required".to_owned(),
            ));
        }
        let wor_url = strip_trailing_slash(wor_url);
        let payload = JoinPayload {
            instance_url: self.string_setting(INSTANCE_URL_KEY).await?,
            uri_prefix: self.string_setting(DATABASE_PREFIX_KEY).await?,
            administrator_email: administrator_email.to_owned(),
            update_endpoint: "updateWebOfRegistries".to_owned(),
            name: self.string_setting(INSTANCE_NAME_KEY).await?,
            description: self.string_setting(FRONT_PAGE_TEXT_KEY).await?,
        };
        let joined = self.client.join(wor_url, &payload).await?;
        self.set_string(ADMINISTRATOR_EMAIL_KEY, administrator_email)
            .await?;
        self.set_string(WEB_OF_REGISTRIES_URL_KEY, wor_url).await?;
        self.set_string(WEB_OF_REGISTRIES_ID_KEY, &joined.id)
            .await?;
        self.set_string(WEB_OF_REGISTRIES_SECRET_KEY, &joined.update_secret)
            .await?;
        Ok(())
    }

    /// Pull the current instance list from the joined Web of Registries and
    /// upsert every `uriPrefix -> instanceUrl` pair into the map (trailing slash
    /// stripped on each instance URL). Returns the number of instances applied.
    /// Not admin-gated: the webhook and the `wor_sync` job drive it.
    pub async fn retrieve(&self) -> Result<usize, FederationError> {
        let wor_url = self.string_setting(WEB_OF_REGISTRIES_URL_KEY).await?;
        if wor_url.is_empty() {
            return Err(FederationError::MissingField(
                "this instance has not joined a Web of Registries".to_owned(),
            ));
        }
        let instances = self
            .client
            .fetch_instances(strip_trailing_slash(&wor_url))
            .await?;
        let mut map = self.registry_map().await?;
        for instance in &instances {
            map.insert(
                instance.uri_prefix.clone(),
                Value::String(strip_trailing_slash(&instance.instance_url).to_owned()),
            );
        }
        self.config
            .set(WEB_OF_REGISTRIES_KEY, &Value::Object(map))
            .await?;
        Ok(instances.len())
    }

    /// The current `uriPrefix -> instanceUrl` map as `(prefix, url)` pairs.
    pub async fn registries(&self) -> Result<Vec<(String, String)>, DomainError> {
        let map = self.registry_map().await?;
        Ok(map
            .into_iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_owned())))
            .collect())
    }

    /// The Web of Registries URL this instance is joined to, or the default one
    /// when it has not joined any.
    pub async fn web_of_registries_url(&self) -> Result<String, DomainError> {
        let url = self.string_setting(WEB_OF_REGISTRIES_URL_KEY).await?;
        Ok(if url.is_empty() {
            DEFAULT_WEB_OF_REGISTRIES_URL.to_owned()
        } else {
            url
        })
    }

    /// Whether this instance has joined a Web of Registries (it carries an id).
    pub async fn is_registered(&self) -> Result<bool, DomainError> {
        Ok(!self
            .string_setting(WEB_OF_REGISTRIES_ID_KEY)
            .await?
            .is_empty())
    }

    /// Upsert a single `uri -> url` registry entry. Admin-gated.
    pub async fn save_registry(
        &self,
        is_admin: bool,
        uri: &str,
        url: &str,
    ) -> Result<(), FederationError> {
        if !is_admin {
            return Err(FederationError::NotAuthorized);
        }
        if url.trim().is_empty() {
            return Err(FederationError::MissingField(
                "a valid registry URL is required".to_owned(),
            ));
        }
        if uri.trim().is_empty() {
            return Err(FederationError::MissingField(
                "a valid registry URI is required".to_owned(),
            ));
        }
        let mut map = self.registry_map().await?;
        map.insert(uri.to_owned(), Value::String(url.to_owned()));
        self.config
            .set(WEB_OF_REGISTRIES_KEY, &Value::Object(map))
            .await?;
        Ok(())
    }

    /// Remove a registry entry by URI. Admin-gated; `404` if absent.
    pub async fn delete_registry(&self, is_admin: bool, uri: &str) -> Result<(), FederationError> {
        if !is_admin {
            return Err(FederationError::NotAuthorized);
        }
        if uri.trim().is_empty() {
            return Err(FederationError::MissingField(
                "a valid registry URI is required".to_owned(),
            ));
        }
        let mut map = self.registry_map().await?;
        if map.remove(uri).is_none() {
            return Err(FederationError::NotFound(format!(
                "registry URI not found: {uri}"
            )));
        }
        self.config
            .set(WEB_OF_REGISTRIES_KEY, &Value::Object(map))
            .await?;
        Ok(())
    }

    /// The configured ICE / Benchling remotes, keyed by id.
    pub async fn remotes(&self) -> Result<Map<String, Value>, DomainError> {
        self.remotes_map().await
    }

    /// Upsert a remote (ICE or Benchling) config, keyed by its `id`. The body is
    /// stored verbatim after validating `type` is `ice` or `benchling` and `id`
    /// is present. Admin-gated.
    pub async fn save_remote(
        &self,
        is_admin: bool,
        remote: Value,
    ) -> Result<String, FederationError> {
        if !is_admin {
            return Err(FederationError::NotAuthorized);
        }
        let remote_type = remote.get("type").and_then(Value::as_str).unwrap_or("");
        if remote_type != "ice" && remote_type != "benchling" {
            return Err(FederationError::MissingField(
                "a valid remote type (benchling/ice) is required".to_owned(),
            ));
        }
        let id = remote
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                FederationError::MissingField("a valid remote id is required".to_owned())
            })?
            .to_owned();
        let mut map = self.remotes_map().await?;
        map.insert(id.clone(), remote);
        self.config.set(REMOTES_KEY, &Value::Object(map)).await?;
        Ok(id)
    }

    /// Remove a remote by id. Admin-gated; `404` if absent.
    pub async fn delete_remote(&self, is_admin: bool, id: &str) -> Result<(), FederationError> {
        if !is_admin {
            return Err(FederationError::NotAuthorized);
        }
        if id.trim().is_empty() {
            return Err(FederationError::MissingField(
                "a valid remote id is required".to_owned(),
            ));
        }
        let mut map = self.remotes_map().await?;
        if map.remove(id).is_none() {
            return Err(FederationError::NotFound(format!("remote not found: {id}")));
        }
        self.config.set(REMOTES_KEY, &Value::Object(map)).await?;
        Ok(())
    }

    /// The update secret that authenticates the `updateWebOfRegistries` webhook,
    /// or `None` when this instance has not joined a Web of Registries.
    pub async fn update_secret(&self) -> Result<Option<String>, DomainError> {
        let value = self.config.get(WEB_OF_REGISTRIES_SECRET_KEY).await?;
        Ok(value
            .and_then(|v| v.as_str().map(str::to_owned))
            .filter(|s| !s.is_empty()))
    }

    /// The most-specific registered instance URL whose prefix `uri` starts with,
    /// or `None` when no registered prefix matches.
    pub async fn resolve_instance(&self, uri: &str) -> Result<Option<String>, DomainError> {
        let pairs = self.registries().await?;
        Ok(longest_prefix_match(&pairs, uri))
    }

    /// Read a config string, falling back to the matching environment variable
    /// then to an empty string, so a fresh instance still federates with
    /// whatever the deployment supplies.
    async fn string_setting(&self, key: &str) -> Result<String, DomainError> {
        if let Some(value) = self.config.get(key).await? {
            if let Some(s) = value.as_str() {
                if !s.is_empty() {
                    return Ok(s.to_owned());
                }
            }
        }
        Ok(std::env::var(env_var_for(key)).unwrap_or_default())
    }

    async fn set_string(&self, key: &str, value: &str) -> Result<(), DomainError> {
        self.config.set(key, &Value::String(value.to_owned())).await
    }

    async fn registry_map(&self) -> Result<Map<String, Value>, DomainError> {
        Ok(match self.config.get(WEB_OF_REGISTRIES_KEY).await? {
            Some(Value::Object(map)) => map,
            _ => Map::new(),
        })
    }

    async fn remotes_map(&self) -> Result<Map<String, Value>, DomainError> {
        Ok(match self.config.get(REMOTES_KEY).await? {
            Some(Value::Object(map)) => map,
            _ => Map::new(),
        })
    }
}

#[async_trait]
impl RemoteObjectResolver for FederationService {
    async fn instances(&self) -> Result<Vec<(String, String)>, DomainError> {
        self.registries().await
    }

    async fn fetch_remote(&self, uri: &str) -> Result<Vec<Triple>, DomainError> {
        let object_url = format!("{uri}{REMOTE_SBOL_SUFFIX}");
        let body = self
            .client
            .fetch_sbol(&object_url)
            .await
            .map_err(|e| DomainError::Parse(e.to_string()))?;
        parse_rdf(&body)
    }
}

/// Parse an RDF/XML document into domain triples, dropping graph tags so the
/// remote object's facts splice into the bare closure set.
fn parse_rdf(body: &str) -> Result<Vec<Triple>, DomainError> {
    if body.trim().is_empty() {
        return Ok(Vec::new());
    }
    let graph = sbol_rdf::Graph::parse(body, sbol_rdf::RdfFormat::RdfXml)
        .map_err(|e| DomainError::Parse(e.to_string()))?;
    let placeholder = IriString::unchecked("");
    let mut triples = sbol_db_rdf::rdf_graph_to_triples(&graph, &placeholder);
    for triple in &mut triples {
        triple.graph_iri = None;
    }
    Ok(triples)
}

/// The most-specific instance URL whose prefix `uri` starts with.
fn longest_prefix_match(pairs: &[(String, String)], uri: &str) -> Option<String> {
    pairs
        .iter()
        .filter(|(prefix, _)| uri.starts_with(prefix.as_str()))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, url)| url.clone())
}

/// Trim a single trailing `/`, matching classic's URL normalization.
fn strip_trailing_slash(url: &str) -> &str {
    url.strip_suffix('/').unwrap_or(url)
}

/// The environment variable a config string falls back to when unset.
fn env_var_for(key: &str) -> String {
    let mut var = String::from("SBOL_DB_");
    for ch in key.chars() {
        if ch.is_ascii_uppercase() {
            var.push('_');
            var.push(ch);
        } else {
            var.push(ch.to_ascii_uppercase());
        }
    }
    var
}

/// Reject a URL that is not a public HTTPS endpoint, the same guard the
/// remote-import job applies: HTTPS only, no embedded credentials, and no
/// private / loopback / link-local host.
pub(crate) fn validate_public_https_url(raw: &str) -> Result<Url, FederationError> {
    let url = Url::parse(raw).map_err(|e| {
        FederationError::RemoteContact(format!("invalid federation URL `{raw}`: {e}"))
    })?;
    if url.scheme() != "https" {
        return Err(FederationError::RemoteContact(
            "federation URL must use https".to_owned(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(FederationError::RemoteContact(
            "federation URL must not contain credentials".to_owned(),
        ));
    }
    let host = url.host_str().ok_or_else(|| {
        FederationError::RemoteContact("federation URL must include a host".to_owned())
    })?;
    validate_public_host(host)?;
    Ok(url)
}

fn validate_public_host(host: &str) -> Result<(), FederationError> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let ip_host = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host.as_str());
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
    {
        return Err(FederationError::RemoteContact(format!(
            "federation host `{host}` is not public"
        )));
    }
    if let Ok(ip) = ip_host.parse::<IpAddr>() {
        validate_public_ip(ip)?;
    }
    Ok(())
}

fn validate_public_ip(ip: IpAddr) -> Result<(), FederationError> {
    let private = match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_multicast()
        }
    };
    if private {
        return Err(FederationError::RemoteContact(format!(
            "federation IP `{ip}` is not public"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;
    use crate::memory::InMemoryConfigStore;

    /// A stub Web of Registries that returns a canned instance list and records
    /// the join payload, so the service is exercised with no network.
    #[derive(Default)]
    struct StubClient {
        instances: Vec<WorInstance>,
        joined: Mutex<Option<(String, JoinPayload)>>,
    }

    #[async_trait]
    impl WebOfRegistriesClient for StubClient {
        async fn join(
            &self,
            wor_url: &str,
            payload: &JoinPayload,
        ) -> Result<JoinResponse, FederationError> {
            *self.joined.lock().unwrap() = Some((wor_url.to_owned(), payload.clone()));
            Ok(JoinResponse {
                id: "instance-7".to_owned(),
                update_secret: "sekret".to_owned(),
            })
        }

        async fn fetch_instances(
            &self,
            _wor_url: &str,
        ) -> Result<Vec<WorInstance>, FederationError> {
            Ok(self.instances.clone())
        }

        async fn fetch_sbol(&self, _object_url: &str) -> Result<String, FederationError> {
            Ok(String::new())
        }
    }

    fn service_with(client: StubClient) -> (FederationService, Arc<dyn ConfigStore>) {
        let store: Arc<dyn ConfigStore> = Arc::new(InMemoryConfigStore::new());
        let svc = FederationService::new(store.clone(), Arc::new(client));
        (svc, store)
    }

    #[tokio::test]
    async fn federate_then_sync_stores_the_prefix_map() {
        let client = StubClient {
            instances: vec![
                WorInstance {
                    uri_prefix: "https://a.org/".to_owned(),
                    instance_url: "https://a.org/".to_owned(),
                },
                WorInstance {
                    uri_prefix: "https://b.org/".to_owned(),
                    instance_url: "https://b.org".to_owned(),
                },
            ],
            ..StubClient::default()
        };
        let (svc, store) = service_with(client);

        // Join: an admin caller stores the returned id, secret, and URL.
        svc.federate(true, "admin@example.org", "https://wor.example.org/")
            .await
            .expect("federate");
        assert_eq!(
            store
                .get(WEB_OF_REGISTRIES_SECRET_KEY)
                .await
                .unwrap()
                .unwrap(),
            json!("sekret")
        );
        assert_eq!(
            store.get(WEB_OF_REGISTRIES_URL_KEY).await.unwrap().unwrap(),
            json!("https://wor.example.org"),
            "the joined URL is stored with its trailing slash stripped"
        );
        assert_eq!(
            svc.update_secret().await.unwrap(),
            Some("sekret".to_owned())
        );

        // Sync: the instance list becomes the prefix -> url map, trailing slash
        // stripped on each instance URL.
        let applied = svc.retrieve().await.expect("retrieve");
        assert_eq!(applied, 2);
        let map = svc.registries().await.expect("registries");
        let map: std::collections::HashMap<_, _> = map.into_iter().collect();
        assert_eq!(map["https://a.org/"], "https://a.org");
        assert_eq!(map["https://b.org/"], "https://b.org");
    }

    #[tokio::test]
    async fn non_admin_mutations_are_refused() {
        let (svc, store) = service_with(StubClient::default());

        assert!(matches!(
            svc.federate(false, "a@b.org", "https://wor.example.org")
                .await,
            Err(FederationError::NotAuthorized)
        ));
        assert!(matches!(
            svc.save_registry(false, "https://x.org/", "https://x.org")
                .await,
            Err(FederationError::NotAuthorized)
        ));
        assert!(matches!(
            svc.save_remote(false, json!({ "id": "r", "type": "ice" }))
                .await,
            Err(FederationError::NotAuthorized)
        ));
        // Nothing was written by the refused calls.
        assert!(store
            .get(WEB_OF_REGISTRIES_URL_KEY)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn registry_save_and_delete_roundtrip() {
        let (svc, _store) = service_with(StubClient::default());

        svc.save_registry(true, "https://x.org/", "https://x.org")
            .await
            .expect("save");
        assert_eq!(
            svc.resolve_instance("https://x.org/public/foo/1")
                .await
                .unwrap(),
            Some("https://x.org".to_owned())
        );

        svc.delete_registry(true, "https://x.org/")
            .await
            .expect("delete");
        assert!(matches!(
            svc.delete_registry(true, "https://x.org/").await,
            Err(FederationError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn remote_save_validates_type_and_id() {
        let (svc, _store) = service_with(StubClient::default());

        // Unknown type is rejected.
        assert!(matches!(
            svc.save_remote(true, json!({ "id": "r1", "type": "sabre" }))
                .await,
            Err(FederationError::MissingField(_))
        ));
        // Missing id is rejected.
        assert!(matches!(
            svc.save_remote(true, json!({ "type": "ice" })).await,
            Err(FederationError::MissingField(_))
        ));
        // A valid ICE remote is stored and read back by id.
        let id = svc
            .save_remote(
                true,
                json!({ "id": "lab-ice", "type": "ice", "url": "https://ice.example.org" }),
            )
            .await
            .expect("save remote");
        assert_eq!(id, "lab-ice");
        let remotes = svc.remotes().await.unwrap();
        assert_eq!(remotes["lab-ice"]["type"], "ice");

        svc.delete_remote(true, "lab-ice").await.expect("delete");
        assert!(matches!(
            svc.delete_remote(true, "lab-ice").await,
            Err(FederationError::NotFound(_))
        ));
    }

    #[test]
    fn longest_prefix_match_prefers_the_more_specific_instance() {
        let pairs = vec![
            ("https://a.org/".to_owned(), "https://a.org".to_owned()),
            (
                "https://a.org/public/".to_owned(),
                "https://mirror.a.org".to_owned(),
            ),
        ];
        assert_eq!(
            longest_prefix_match(&pairs, "https://a.org/public/foo/1"),
            Some("https://mirror.a.org".to_owned())
        );
        assert_eq!(
            longest_prefix_match(&pairs, "https://a.org/user/x/1"),
            Some("https://a.org".to_owned())
        );
        assert_eq!(longest_prefix_match(&pairs, "https://other.org/x"), None);
    }

    #[test]
    fn ssrf_guard_rejects_non_public_urls() {
        for raw in [
            "http://wor.example.org/instances",
            "https://localhost/instances",
            "https://127.0.0.1/instances",
            "https://[::1]/instances",
            "https://10.0.0.5/instances",
        ] {
            assert!(validate_public_https_url(raw).is_err(), "{raw}");
        }
        assert!(validate_public_https_url("https://wor.example.org/instances").is_ok());
    }
}
