use async_trait::async_trait;
use openraft::Raft;
use sbol_db_core::{DomainError, UserId};
use sbol_db_storage::TokenStore;
use uuid::Uuid;

use crate::{
    CommandEnvelope, CommandOutcome, CommandResult, ReplicatedCommand, RocksStateMachine,
    TypeConfig,
};

/// Leader-only token persistence over the replicated RocksDB state machine.
///
/// Only the application's one-way token hash is logged. Linearizable resolve
/// calls never consult a follower that may lag revocation.
#[derive(Clone)]
pub struct ReplicatedTokenStore {
    raft: Raft<TypeConfig>,
    state_machine: RocksStateMachine,
    client_id: Uuid,
}

impl ReplicatedTokenStore {
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
    ) -> Result<CommandResult, DomainError> {
        let response = self
            .raft
            .client_write(CommandEnvelope::new(self.client_id, request_id, command))
            .await
            .map_err(consensus_error)?
            .data;
        match response.outcome {
            CommandOutcome::Applied => Ok(response.result),
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

    pub async fn issue_with_request_id(
        &self,
        request_id: Uuid,
        token_hash: &str,
        user_id: UserId,
    ) -> Result<(), DomainError> {
        match self
            .submit(
                request_id,
                ReplicatedCommand::IssueToken {
                    token_hash: token_hash.to_owned(),
                    user_id,
                },
            )
            .await?
        {
            CommandResult::Unit => Ok(()),
            result => Err(unexpected_result("issue token", result)),
        }
    }

    pub async fn revoke_with_request_id(
        &self,
        request_id: Uuid,
        token_hash: &str,
    ) -> Result<bool, DomainError> {
        match self
            .submit(
                request_id,
                ReplicatedCommand::RevokeToken {
                    token_hash: token_hash.to_owned(),
                },
            )
            .await?
        {
            CommandResult::Bool(existed) => Ok(existed),
            result => Err(unexpected_result("revoke token", result)),
        }
    }
}

#[async_trait]
impl TokenStore for ReplicatedTokenStore {
    async fn issue(&self, token_hash: &str, user_id: UserId) -> Result<(), DomainError> {
        self.issue_with_request_id(Uuid::new_v4(), token_hash, user_id)
            .await
    }

    async fn resolve(&self, token_hash: &str) -> Result<Option<UserId>, DomainError> {
        self.linearizable_read().await?;
        self.state_machine
            .resolve_token(token_hash)
            .map_err(consensus_error)
    }

    async fn revoke(&self, token_hash: &str) -> Result<bool, DomainError> {
        self.revoke_with_request_id(Uuid::new_v4(), token_hash)
            .await
    }
}

fn unexpected_result(operation: &str, result: CommandResult) -> DomainError {
    DomainError::Database(format!(
        "Raft state machine returned {result:?} for {operation}"
    ))
}

fn consensus_error(error: impl std::fmt::Display) -> DomainError {
    DomainError::Database(format!("Raft consensus: {error}"))
}
