//! Durable administrator settings for the self-contained edge runtime.
//!
//! The active process configuration is immutable. Administrator edits are
//! persisted in the backend configuration store, included in the next RocksDB
//! checkpoint, and applied on a deliberate restart. This keeps TLS listeners,
//! object-store clients, and disk admission policy internally consistent.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use age::x25519;
use serde::{Deserialize, Serialize};
use url::{Host, Url};
use uuid::Uuid;

use sbol_db_core::DomainError;
use sbol_db_storage::ConfigStore;

use crate::metrics::EdgeHealthSnapshot;
use crate::Metrics;

pub const EDGE_SETTINGS_KEY: &str = "edge_runtime";
pub const EDGE_SETTINGS_VERSION: u32 = 1;
pub const MIN_BACKUP_INTERVAL_SECS: u64 = 15 * 60;
pub const MAX_BACKUP_INTERVAL_SECS: u64 = 30 * 24 * 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeSettings {
    pub version: u32,
    pub hostname: String,
    pub acme_contact: String,
    pub acme_directory_url: String,
    pub http_redirect_enabled: bool,
    pub tls_handshake_timeout_secs: u64,
    pub backup_recovery_recipient: String,
    pub backup_repository_url: String,
    pub backup_interval_secs: u64,
    pub backup_local_retention: usize,
    pub minimum_free_bytes: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EdgeSettingsPatch {
    pub hostname: Option<String>,
    pub acme_contact: Option<String>,
    pub acme_directory_url: Option<String>,
    pub http_redirect_enabled: Option<bool>,
    pub tls_handshake_timeout_secs: Option<u64>,
    pub backup_recovery_recipient: Option<String>,
    pub backup_repository_url: Option<String>,
    pub backup_interval_secs: Option<u64>,
    pub backup_local_retention: Option<usize>,
    pub minimum_free_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EdgeRuntimeIdentity {
    pub profile: &'static str,
    pub layout_version: String,
    pub generation: Uuid,
    pub data_dir: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct EdgeAdminSnapshot {
    pub active: EdgeSettings,
    pub pending: EdgeSettings,
    pub restart_required: bool,
    pub runtime: EdgeRuntimeIdentity,
    pub health: EdgeHealthSnapshot,
}

#[derive(Debug, thiserror::Error)]
pub enum EdgeAdminError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Storage(#[from] DomainError),
}

#[derive(Clone)]
pub struct EdgeAdminService {
    store: Arc<dyn ConfigStore>,
    active: EdgeSettings,
    runtime: EdgeRuntimeIdentity,
    metrics: Arc<Metrics>,
}

impl fmt::Debug for EdgeAdminService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EdgeAdminService")
            .field("active", &self.active)
            .field("runtime", &self.runtime)
            .finish_non_exhaustive()
    }
}

impl EdgeSettings {
    pub fn validate(mut self) -> Result<Self, EdgeAdminError> {
        if self.version != EDGE_SETTINGS_VERSION {
            return Err(EdgeAdminError::Invalid(format!(
                "unsupported edge settings version {}; expected {EDGE_SETTINGS_VERSION}",
                self.version
            )));
        }
        self.hostname = normalize_hostname(&self.hostname)?;
        self.acme_contact = normalize_contact(&self.acme_contact)?;
        self.acme_directory_url = validate_acme_directory(&self.acme_directory_url)?;
        self.backup_recovery_recipient = self.backup_recovery_recipient.trim().to_owned();
        x25519::Recipient::from_str(&self.backup_recovery_recipient).map_err(|error| {
            EdgeAdminError::Invalid(format!("invalid age recovery recipient: {error}"))
        })?;
        self.backup_repository_url = validate_repository_url(&self.backup_repository_url)?;
        if !(1..=60).contains(&self.tls_handshake_timeout_secs) {
            return Err(EdgeAdminError::Invalid(
                "TLS handshake timeout must be between 1 and 60 seconds".to_owned(),
            ));
        }
        if !(MIN_BACKUP_INTERVAL_SECS..=MAX_BACKUP_INTERVAL_SECS)
            .contains(&self.backup_interval_secs)
        {
            return Err(EdgeAdminError::Invalid(format!(
                "backup interval must be between {MIN_BACKUP_INTERVAL_SECS} and {MAX_BACKUP_INTERVAL_SECS} seconds"
            )));
        }
        if !(1..=30).contains(&self.backup_local_retention) {
            return Err(EdgeAdminError::Invalid(
                "local backup retention must be between 1 and 30 artifacts".to_owned(),
            ));
        }
        if self.minimum_free_bytes == 0 {
            return Err(EdgeAdminError::Invalid(
                "minimum free bytes must be greater than zero".to_owned(),
            ));
        }
        Ok(self)
    }

    pub fn apply_patch(mut self, patch: EdgeSettingsPatch) -> Result<Self, EdgeAdminError> {
        macro_rules! apply {
            ($field:ident) => {
                if let Some(value) = patch.$field {
                    self.$field = value;
                }
            };
        }
        apply!(hostname);
        apply!(acme_contact);
        apply!(acme_directory_url);
        apply!(http_redirect_enabled);
        apply!(tls_handshake_timeout_secs);
        apply!(backup_recovery_recipient);
        apply!(backup_repository_url);
        apply!(backup_interval_secs);
        apply!(backup_local_retention);
        apply!(minimum_free_bytes);
        self.validate()
    }
}

impl EdgeAdminService {
    pub fn new(
        store: Arc<dyn ConfigStore>,
        active: EdgeSettings,
        runtime: EdgeRuntimeIdentity,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            store,
            active,
            runtime,
            metrics,
        }
    }

    pub async fn snapshot(&self) -> Result<EdgeAdminSnapshot, EdgeAdminError> {
        let pending = read_edge_settings(self.store.as_ref())
            .await?
            .unwrap_or_else(|| self.active.clone());
        Ok(EdgeAdminSnapshot {
            restart_required: pending != self.active,
            active: self.active.clone(),
            pending,
            runtime: self.runtime.clone(),
            health: self.metrics.edge_health_snapshot(),
        })
    }

    pub async fn update(
        &self,
        patch: EdgeSettingsPatch,
    ) -> Result<EdgeAdminSnapshot, EdgeAdminError> {
        let current = read_edge_settings(self.store.as_ref())
            .await?
            .unwrap_or_else(|| self.active.clone());
        let pending = current.apply_patch(patch)?;
        write_edge_settings(self.store.as_ref(), &pending).await?;
        self.snapshot().await
    }
}

pub async fn read_edge_settings(
    store: &dyn ConfigStore,
) -> Result<Option<EdgeSettings>, EdgeAdminError> {
    let Some(value) = store.get(EDGE_SETTINGS_KEY).await? else {
        return Ok(None);
    };
    let settings = serde_json::from_value::<EdgeSettings>(value).map_err(|error| {
        EdgeAdminError::Invalid(format!("stored edge settings are invalid: {error}"))
    })?;
    settings.validate().map(Some)
}

pub async fn write_edge_settings(
    store: &dyn ConfigStore,
    settings: &EdgeSettings,
) -> Result<(), EdgeAdminError> {
    let settings = settings.clone().validate()?;
    let value = serde_json::to_value(settings).map_err(|error| {
        EdgeAdminError::Invalid(format!("cannot encode edge settings: {error}"))
    })?;
    store.set(EDGE_SETTINGS_KEY, &value).await?;
    Ok(())
}

fn normalize_hostname(raw: &str) -> Result<String, EdgeAdminError> {
    let raw = raw.trim().trim_end_matches('.');
    if raw.is_empty() || raw.contains('*') {
        return Err(EdgeAdminError::Invalid(
            "hostname must be one concrete DNS name, not empty or a wildcard".to_owned(),
        ));
    }
    match Host::parse(raw) {
        Ok(Host::Domain(domain)) if !domain.is_empty() => Ok(domain.to_ascii_lowercase()),
        Ok(Host::Domain(_)) => Err(EdgeAdminError::Invalid(
            "hostname cannot be empty".to_owned(),
        )),
        Ok(Host::Ipv4(_) | Host::Ipv6(_)) => Err(EdgeAdminError::Invalid(
            "hostname must be a DNS name, not an IP address".to_owned(),
        )),
        Err(error) => Err(EdgeAdminError::Invalid(format!(
            "hostname is invalid: {error}"
        ))),
    }
}

fn normalize_contact(raw: &str) -> Result<String, EdgeAdminError> {
    let email = raw.trim().strip_prefix("mailto:").unwrap_or(raw.trim());
    if email
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return Err(EdgeAdminError::Invalid(
            "ACME contact must be one email address without whitespace".to_owned(),
        ));
    }
    let Some((local, domain)) = email.split_once('@') else {
        return Err(EdgeAdminError::Invalid(
            "ACME contact must be an email address".to_owned(),
        ));
    };
    if local.is_empty() || domain.is_empty() || domain.contains('@') {
        return Err(EdgeAdminError::Invalid(
            "ACME contact must be an email address".to_owned(),
        ));
    }
    normalize_hostname(domain)?;
    Ok(email.to_owned())
}

fn validate_acme_directory(raw: &str) -> Result<String, EdgeAdminError> {
    let url = Url::parse(raw.trim()).map_err(|error| {
        EdgeAdminError::Invalid(format!("ACME directory URL is invalid: {error}"))
    })?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(EdgeAdminError::Invalid(
            "ACME directory must be an HTTPS URL without credentials or a fragment".to_owned(),
        ));
    }
    Ok(url.into())
}

fn validate_repository_url(raw: &str) -> Result<String, EdgeAdminError> {
    let url = Url::parse(raw.trim()).map_err(|error| {
        EdgeAdminError::Invalid(format!("backup repository URL is invalid: {error}"))
    })?;
    if !matches!(url.scheme(), "s3" | "gs") {
        return Err(EdgeAdminError::Invalid(
            "backup repository must use s3:// or gs://".to_owned(),
        ));
    }
    if url.host_str().is_none()
        || url.path().trim_matches('/').is_empty()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(EdgeAdminError::Invalid(
            "backup repository must contain only a bucket and non-empty instance prefix".to_owned(),
        ));
    }
    Ok(url.into())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sbol_db_app::memory::InMemoryConfigStore;

    use super::*;

    fn settings() -> EdgeSettings {
        EdgeSettings {
            version: EDGE_SETTINGS_VERSION,
            hostname: "Registry.Example.org.".to_owned(),
            acme_contact: "mailto:admin@example.org".to_owned(),
            acme_directory_url: "https://acme.example.org/directory".to_owned(),
            http_redirect_enabled: true,
            tls_handshake_timeout_secs: 10,
            backup_recovery_recipient: x25519::Identity::generate().to_public().to_string(),
            backup_repository_url: "s3://backups/registry/production".to_owned(),
            backup_interval_secs: 86_400,
            backup_local_retention: 2,
            minimum_free_bytes: 2_147_483_648,
        }
    }

    #[test]
    fn validates_and_normalizes_edge_settings() {
        let settings = settings().validate().unwrap();
        assert_eq!(settings.hostname, "registry.example.org");
        assert_eq!(settings.acme_contact, "admin@example.org");
    }

    #[test]
    fn rejects_credentials_and_missing_repository_prefix() {
        for repository in ["s3://bucket", "s3://key:secret@bucket/registry"] {
            let mut candidate = settings();
            candidate.backup_repository_url = repository.to_owned();
            assert!(candidate.validate().is_err(), "accepted {repository}");
        }
    }

    #[tokio::test]
    async fn persists_pending_settings_in_the_backend_config_store() {
        let store = Arc::new(InMemoryConfigStore::new());
        let active = settings().validate().unwrap();
        write_edge_settings(store.as_ref(), &active).await.unwrap();
        let loaded = read_edge_settings(store.as_ref()).await.unwrap().unwrap();
        assert_eq!(loaded, active);

        let pending = loaded
            .apply_patch(EdgeSettingsPatch {
                backup_interval_secs: Some(3_600),
                ..EdgeSettingsPatch::default()
            })
            .unwrap();
        assert_eq!(pending.backup_interval_secs, 3_600);
    }
}
