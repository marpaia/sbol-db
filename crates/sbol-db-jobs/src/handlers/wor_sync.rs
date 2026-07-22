//! `wor_sync` job handler.
//!
//! Pulls the current instance list from the joined Web of Registries and
//! upserts every `uriPrefix -> instanceUrl` pair into the durable config store,
//! the job-runtime counterpart to the app-layer
//! `FederationService::retrieve`. Running it as a job lets the public
//! `updateWebOfRegistries` webhook return immediately and lets an operator
//! schedule periodic re-syncs off the request path.
//!
//! The config keys and instance wire shape mirror the app-layer federation
//! contract; this crate cannot depend on the app facade, so the small shared
//! surface is restated here.

use async_trait::async_trait;
use sbol_db_storage::ConfigStore;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::context::JobContext;
use crate::handler::{HandlerError, JobHandler, JobOutcome};
use crate::handlers::import_remote_document::validate_public_https_url;

pub const KIND: &str = "wor_sync";

/// The config key holding the `uriPrefix -> instanceUrl` map.
const WEB_OF_REGISTRIES_KEY: &str = "webOfRegistries";
/// The config key holding the joined Web of Registries base URL.
const WEB_OF_REGISTRIES_URL_KEY: &str = "webOfRegistriesUrl";

/// One registry instance as advertised by the Web of Registries `/instances`
/// endpoint.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct WorInstance {
    #[serde(rename = "uriPrefix")]
    uri_prefix: String,
    #[serde(rename = "instanceUrl")]
    instance_url: String,
}

/// The `wor_sync` payload carries no fields: the joined URL comes from the
/// config store so a scheduled trigger needs no arguments.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorSyncPayload {}

pub struct WorSyncHandler;

#[async_trait]
impl JobHandler for WorSyncHandler {
    type Payload = WorSyncPayload;

    fn kind(&self) -> &'static str {
        KIND
    }

    async fn run(
        &self,
        ctx: JobContext,
        _payload: Self::Payload,
    ) -> Result<JobOutcome, HandlerError> {
        let config = ctx.config.clone().ok_or_else(|| {
            HandlerError::Other("wor_sync requires a worker configured with a config store".into())
        })?;

        let wor_url = read_string(config.as_ref(), WEB_OF_REGISTRIES_URL_KEY)
            .await?
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                HandlerError::Other("this instance has not joined a Web of Registries".into())
            })?;
        let url =
            validate_public_https_url(&format!("{}/instances", strip_trailing_slash(&wor_url)))
                .map_err(|e| HandlerError::Other(e.to_string()))?;

        ctx.log(
            "info",
            "web of registries sync starting",
            serde_json::json!({ "url": url.as_str() }),
        )
        .await;

        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("sbol-db/", env!("CARGO_PKG_VERSION"), " wor sync"))
            .build()
            .map_err(|e| HandlerError::Other(format!("wor sync client build failed: {e}")))?;
        let body = client
            .get(url.clone())
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| HandlerError::Other(format!("wor sync fetch failed: {e}")))?
            .text()
            .await
            .map_err(|e| HandlerError::Other(format!("wor sync read failed: {e}")))?;

        let instances: Vec<WorInstance> = serde_json::from_str(&body)
            .map_err(|e| HandlerError::Other(format!("wor sync parse failed: {e}")))?;
        let applied = apply_instances(config.as_ref(), &instances).await?;

        ctx.log(
            "info",
            "web of registries sync completed",
            serde_json::json!({ "instances": applied }),
        )
        .await;
        Ok(JobOutcome::with_result(
            serde_json::json!({ "instances": applied }),
        ))
    }
}

/// Upsert every instance's `uriPrefix -> instanceUrl` pair into the map,
/// stripping a trailing slash from each instance URL, then persist it. Returns
/// the number of instances applied. Shared with the handler so it is unit
/// tested without any network.
async fn apply_instances(
    config: &dyn ConfigStore,
    instances: &[WorInstance],
) -> Result<usize, HandlerError> {
    let mut map: Map<String, Value> = match config.get(WEB_OF_REGISTRIES_KEY).await? {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    for instance in instances {
        map.insert(
            instance.uri_prefix.clone(),
            Value::String(strip_trailing_slash(&instance.instance_url).to_owned()),
        );
    }
    config
        .set(WEB_OF_REGISTRIES_KEY, &Value::Object(map))
        .await?;
    Ok(instances.len())
}

async fn read_string(config: &dyn ConfigStore, key: &str) -> Result<Option<String>, HandlerError> {
    Ok(config
        .get(key)
        .await?
        .and_then(|v| v.as_str().map(str::to_owned)))
}

fn strip_trailing_slash(url: &str) -> &str {
    url.strip_suffix('/').unwrap_or(url)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use sbol_db_core::{ConfigEntry, DomainError};
    use serde_json::json;

    use super::*;

    /// A minimal in-memory config store for the handler's upsert path, so the
    /// jobs crate needs no app-layer dependency to test it.
    #[derive(Default)]
    struct MemConfig {
        entries: Mutex<HashMap<String, Value>>,
    }

    #[async_trait]
    impl ConfigStore for MemConfig {
        async fn get(&self, key: &str) -> Result<Option<Value>, DomainError> {
            Ok(self.entries.lock().unwrap().get(key).cloned())
        }
        async fn set(&self, key: &str, value: &Value) -> Result<(), DomainError> {
            self.entries
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.clone());
            Ok(())
        }
        async fn get_all(&self) -> Result<Vec<ConfigEntry>, DomainError> {
            Ok(Vec::new())
        }
        async fn delete(&self, key: &str) -> Result<(), DomainError> {
            self.entries.lock().unwrap().remove(key);
            Ok(())
        }
    }

    #[tokio::test]
    async fn wor_sync_updates_map() {
        let config = MemConfig::default();
        let instances = vec![
            WorInstance {
                uri_prefix: "https://a.org/".to_owned(),
                instance_url: "https://a.org/".to_owned(),
            },
            WorInstance {
                uri_prefix: "https://b.org/".to_owned(),
                instance_url: "https://b.org".to_owned(),
            },
        ];
        let applied = apply_instances(&config, &instances).await.expect("apply");
        assert_eq!(applied, 2);

        let stored = config.get(WEB_OF_REGISTRIES_KEY).await.unwrap().unwrap();
        assert_eq!(
            stored,
            json!({ "https://a.org/": "https://a.org", "https://b.org/": "https://b.org" }),
            "the stored map holds each prefix -> url with the trailing slash stripped"
        );

        // A second sync upserts without dropping existing entries.
        let more = vec![WorInstance {
            uri_prefix: "https://c.org/".to_owned(),
            instance_url: "https://c.org".to_owned(),
        }];
        apply_instances(&config, &more).await.expect("apply again");
        let stored = config.get(WEB_OF_REGISTRIES_KEY).await.unwrap().unwrap();
        assert_eq!(stored["https://a.org/"], "https://a.org");
        assert_eq!(stored["https://c.org/"], "https://c.org");
    }
}
