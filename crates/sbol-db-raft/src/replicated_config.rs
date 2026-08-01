use async_trait::async_trait;
use chrono::Utc;
use openraft::Raft;
use sbol_db_core::{ConfigEntry, DomainError};
use sbol_db_storage::ConfigStore;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    CommandEnvelope, CommandOutcome, CommandResponse, ReplicatedCommand, RocksStateMachine,
    TypeConfig,
};

/// Leader-only `ConfigStore` adapter backed by the Raft state machine.
///
/// Reads first confirm leadership and quorum contact. Writes return only after
/// OpenRaft has committed and applied the command. A follower currently returns
/// a database error containing OpenRaft's leader-forwarding detail; the HTTP
/// integration layer will translate that into an explicit redirect/retry
/// response rather than silently serving stale state.
#[derive(Clone)]
pub struct ReplicatedConfigStore {
    raft: Raft<TypeConfig>,
    state_machine: RocksStateMachine,
    client_id: Uuid,
}

impl ReplicatedConfigStore {
    pub fn new(raft: Raft<TypeConfig>, state_machine: RocksStateMachine, client_id: Uuid) -> Self {
        Self {
            raft,
            state_machine,
            client_id,
        }
    }

    async fn submit(
        &self,
        request_id: Uuid,
        command: ReplicatedCommand,
    ) -> Result<CommandResponse, DomainError> {
        let request = CommandEnvelope::new(self.client_id, request_id, command);
        let response = self
            .raft
            .client_write(request)
            .await
            .map_err(consensus_error)?
            .data;
        match response.outcome {
            CommandOutcome::Applied => Ok(response),
            ref rejected => Err(DomainError::Database(format!(
                "replicated command rejected: {rejected:?}"
            ))),
        }
    }

    async fn linearizable_read(&self) -> Result<(), DomainError> {
        self.raft
            .ensure_linearizable()
            .await
            .map(|_| ())
            .map_err(consensus_error)
    }

    /// Set a value under a caller-supplied idempotency key. Public request
    /// routing should retain this id across timeouts and leader redirects.
    pub async fn set_with_request_id(
        &self,
        request_id: Uuid,
        key: &str,
        value: &Value,
    ) -> Result<(), DomainError> {
        self.submit(
            request_id,
            ReplicatedCommand::SetConfig {
                key: key.to_owned(),
                value: value.clone(),
                updated_at: Utc::now(),
            },
        )
        .await
        .map(|_| ())
    }

    pub async fn delete_with_request_id(
        &self,
        request_id: Uuid,
        key: &str,
    ) -> Result<(), DomainError> {
        self.submit(
            request_id,
            ReplicatedCommand::DeleteConfig {
                key: key.to_owned(),
            },
        )
        .await
        .map(|_| ())
    }
}

#[async_trait]
impl ConfigStore for ReplicatedConfigStore {
    async fn get(&self, key: &str) -> Result<Option<Value>, DomainError> {
        self.linearizable_read().await?;
        self.state_machine
            .read_config(key)
            .map(|entry| entry.map(|entry| entry.value))
            .map_err(consensus_error)
    }

    async fn set(&self, key: &str, value: &Value) -> Result<(), DomainError> {
        self.set_with_request_id(Uuid::new_v4(), key, value).await
    }

    async fn get_all(&self) -> Result<Vec<ConfigEntry>, DomainError> {
        self.linearizable_read().await?;
        self.state_machine
            .read_all_config()
            .map_err(consensus_error)
    }

    async fn delete(&self, key: &str) -> Result<(), DomainError> {
        self.delete_with_request_id(Uuid::new_v4(), key).await
    }
}

fn consensus_error(error: impl std::fmt::Display) -> DomainError {
    DomainError::Database(format!("Raft consensus: {error}"))
}
