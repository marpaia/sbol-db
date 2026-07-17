//! Instance-configuration service.
//!
//! [`ConfigService`] wraps a [`ConfigStore`] with admin-gated mutation: reads
//! are open (classic SynBioHub exposes several config GETs publicly), while a
//! write or delete requires an administrator, matching the `requireAdmin` gate
//! on the classic `/admin` config actions. It is the durable replacement for
//! the mutable `config.local.json`; typed per-section accessors layer on top of
//! its raw key/JSON-value surface.

use std::sync::Arc;

use sbol_db_core::{ConfigEntry, DomainError};
use sbol_db_storage::ConfigStore;
use serde_json::Value;

/// A configuration mutation's failure mode, kept distinct from a plain
/// [`DomainError`] so the adapter maps an authorization failure to `403` rather
/// than `500`.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The caller is not an administrator (or is anonymous), so the mutation is
    /// refused.
    #[error("administrator privileges are required to change configuration")]
    NotAuthorized,
    #[error(transparent)]
    Domain(#[from] DomainError),
}

/// Reads and admin-gated writes over the durable instance configuration.
#[derive(Clone)]
pub struct ConfigService {
    store: Arc<dyn ConfigStore>,
}

impl ConfigService {
    /// Wrap a [`ConfigStore`].
    pub fn new(store: Arc<dyn ConfigStore>) -> Self {
        Self { store }
    }

    /// The value stored under `key`, or `None` when it has never been set.
    /// Reads are not admin-gated.
    pub async fn get(&self, key: &str) -> Result<Option<Value>, DomainError> {
        self.store.get(key).await
    }

    /// Every stored entry. Reads are not admin-gated.
    pub async fn get_all(&self) -> Result<Vec<ConfigEntry>, DomainError> {
        self.store.get_all().await
    }

    /// Write `value` under `key`, upserting. Requires an administrator: a
    /// non-admin caller (`is_admin == false`) is refused with
    /// [`ConfigError::NotAuthorized`] and nothing is written.
    pub async fn set(&self, is_admin: bool, key: &str, value: &Value) -> Result<(), ConfigError> {
        if !is_admin {
            return Err(ConfigError::NotAuthorized);
        }
        self.store.set(key, value).await?;
        Ok(())
    }

    /// Remove the entry under `key`. Requires an administrator, like
    /// [`set`](Self::set).
    pub async fn delete(&self, is_admin: bool, key: &str) -> Result<(), ConfigError> {
        if !is_admin {
            return Err(ConfigError::NotAuthorized);
        }
        self.store.delete(key).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::{ConfigError, ConfigService};
    use crate::memory::InMemoryConfigStore;

    /// A fresh store has no value under a key, and a non-admin caller can never
    /// mutate it.
    #[tokio::test]
    async fn defaults_absent_and_writes_are_admin_gated() {
        let svc = ConfigService::new(Arc::new(InMemoryConfigStore::new()));

        // Unset keys read back as absent.
        assert!(
            svc.get("mail").await.expect("get").is_none(),
            "a fresh store has no value under an unset key"
        );
        assert!(
            svc.get_all().await.expect("get_all").is_empty(),
            "a fresh store enumerates no entries"
        );

        // A non-admin cannot write, and the store stays empty.
        let denied = svc
            .set(false, "mail", &json!({ "fromAddress": "a@b.org" }))
            .await;
        assert!(
            matches!(denied, Err(ConfigError::NotAuthorized)),
            "a non-admin write is refused"
        );
        assert!(
            svc.get("mail").await.expect("get").is_none(),
            "a refused write persists nothing"
        );
    }

    /// An admin write is durable and a later write to the same key overwrites
    /// the earlier value; an admin delete removes it.
    #[tokio::test]
    async fn admin_set_get_overwrite_delete_roundtrip() {
        let svc = ConfigService::new(Arc::new(InMemoryConfigStore::new()));

        let mail = json!({ "fromAddress": "admin@example.org", "sendgridApiKey": "sg-1" });
        svc.set(true, "mail", &mail).await.expect("admin set");
        assert_eq!(
            svc.get("mail").await.expect("get"),
            Some(mail),
            "an admin write reads back verbatim"
        );

        // Upsert: a second write to the same key overwrites the value.
        let mail2 = json!({ "fromAddress": "ops@example.org" });
        svc.set(true, "mail", &mail2)
            .await
            .expect("admin overwrite");
        assert_eq!(
            svc.get("mail").await.expect("get"),
            Some(mail2),
            "a later write overwrites the earlier value"
        );

        // Delete is admin-gated and removes the entry.
        assert!(
            matches!(
                svc.delete(false, "mail").await,
                Err(ConfigError::NotAuthorized)
            ),
            "a non-admin delete is refused"
        );
        svc.delete(true, "mail").await.expect("admin delete");
        assert!(
            svc.get("mail").await.expect("get").is_none(),
            "an admin delete removes the entry"
        );
    }
}
