use async_trait::async_trait;
use chrono::Utc;
use openraft::Raft;
use sbol_db_core::{DomainError, NewUser, User, UserId};
use sbol_db_storage::UserStore;
use uuid::Uuid;

use crate::{
    CommandEnvelope, CommandOutcome, CommandResponse, CommandResult, ReplicatedCommand,
    RocksStateMachine, TypeConfig,
};

/// Leader-only account persistence over the replicated RocksDB state machine.
#[derive(Clone)]
pub struct ReplicatedUserStore {
    raft: Raft<TypeConfig>,
    state_machine: RocksStateMachine,
    client_id: Uuid,
}

impl ReplicatedUserStore {
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
        let response = self
            .raft
            .client_write(CommandEnvelope::new(self.client_id, request_id, command))
            .await
            .map_err(consensus_error)?
            .data;
        match &response.outcome {
            CommandOutcome::Applied => Ok(response),
            CommandOutcome::ConstraintViolation { message } => {
                Err(DomainError::Database(message.clone()))
            }
            CommandOutcome::NotFound { entity } => Err(DomainError::NotFound(entity.clone())),
            rejected => Err(DomainError::Database(format!(
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

    pub async fn create_user_with_request_id(
        &self,
        request_id: Uuid,
        new_user: NewUser,
    ) -> Result<User, DomainError> {
        let now = Utc::now();
        let user = User {
            id: UserId::new(),
            username: new_user.username,
            name: new_user.name,
            email: new_user.email,
            affiliation: new_user.affiliation,
            password_hash: new_user.password_hash,
            graph_uri: new_user.graph_uri,
            is_admin: new_user.is_admin,
            is_curator: new_user.is_curator,
            is_member: new_user.is_member,
            reset_password_link: None,
            created_at: now,
            updated_at: now,
        };
        expect_user(
            "create user",
            self.submit(request_id, ReplicatedCommand::CreateUser { user })
                .await?
                .result,
        )?
        .ok_or_else(|| DomainError::Database("create user returned no account".to_owned()))
    }

    pub async fn update_user_with_request_id(
        &self,
        request_id: Uuid,
        user: &User,
    ) -> Result<User, DomainError> {
        expect_user(
            "update user",
            self.submit(
                request_id,
                ReplicatedCommand::UpdateUserProfile {
                    id: user.id,
                    name: user.name.clone(),
                    affiliation: user.affiliation.clone(),
                    is_admin: user.is_admin,
                    is_curator: user.is_curator,
                    is_member: user.is_member,
                    updated_at: Utc::now(),
                },
            )
            .await?
            .result,
        )?
        .ok_or_else(|| DomainError::Database("update user returned no account".to_owned()))
    }

    pub async fn set_password_hash_with_request_id(
        &self,
        request_id: Uuid,
        id: UserId,
        password_hash: &str,
    ) -> Result<(), DomainError> {
        expect_unit(
            "set user password hash",
            self.submit(
                request_id,
                ReplicatedCommand::SetUserPasswordHash {
                    id,
                    password_hash: password_hash.to_owned(),
                    updated_at: Utc::now(),
                },
            )
            .await?
            .result,
        )
    }

    pub async fn set_reset_link_with_request_id(
        &self,
        request_id: Uuid,
        id: UserId,
        link: Option<&str>,
    ) -> Result<(), DomainError> {
        expect_unit(
            "set user reset link",
            self.submit(
                request_id,
                ReplicatedCommand::SetUserResetLink {
                    id,
                    link: link.map(str::to_owned),
                },
            )
            .await?
            .result,
        )
    }

    pub async fn consume_reset_link_with_request_id(
        &self,
        request_id: Uuid,
        link: &str,
    ) -> Result<Option<User>, DomainError> {
        expect_user(
            "consume user reset link",
            self.submit(
                request_id,
                ReplicatedCommand::ConsumeUserResetLink {
                    link: link.to_owned(),
                },
            )
            .await?
            .result,
        )
    }

    pub async fn delete_user_with_request_id(
        &self,
        request_id: Uuid,
        id: UserId,
    ) -> Result<bool, DomainError> {
        expect_bool(
            "delete user",
            self.submit(request_id, ReplicatedCommand::DeleteUser { id })
                .await?
                .result,
        )
    }
}

#[async_trait]
impl UserStore for ReplicatedUserStore {
    async fn create_user(&self, new_user: NewUser) -> Result<User, DomainError> {
        self.create_user_with_request_id(Uuid::new_v4(), new_user)
            .await
    }

    async fn find_by_email_or_username(
        &self,
        identifier: &str,
    ) -> Result<Option<User>, DomainError> {
        self.linearizable_read().await?;
        self.state_machine
            .find_user(identifier)
            .map_err(consensus_error)
    }

    async fn get_by_id(&self, id: UserId) -> Result<Option<User>, DomainError> {
        self.linearizable_read().await?;
        self.state_machine.read_user(id).map_err(consensus_error)
    }

    async fn list_users(&self) -> Result<Vec<User>, DomainError> {
        self.linearizable_read().await?;
        self.state_machine.read_all_users().map_err(consensus_error)
    }

    async fn update_user(&self, user: &User) -> Result<User, DomainError> {
        self.update_user_with_request_id(Uuid::new_v4(), user).await
    }

    async fn set_password_hash(&self, id: UserId, password_hash: &str) -> Result<(), DomainError> {
        self.set_password_hash_with_request_id(Uuid::new_v4(), id, password_hash)
            .await
    }

    async fn set_reset_link(&self, id: UserId, link: Option<&str>) -> Result<(), DomainError> {
        self.set_reset_link_with_request_id(Uuid::new_v4(), id, link)
            .await
    }

    async fn consume_reset_link(&self, link: &str) -> Result<Option<User>, DomainError> {
        self.consume_reset_link_with_request_id(Uuid::new_v4(), link)
            .await
    }

    async fn delete_user(&self, id: UserId) -> Result<bool, DomainError> {
        self.delete_user_with_request_id(Uuid::new_v4(), id).await
    }

    async fn any_admin(&self) -> Result<bool, DomainError> {
        self.linearizable_read().await?;
        self.state_machine.has_admin().map_err(consensus_error)
    }
}

fn expect_unit(operation: &str, result: CommandResult) -> Result<(), DomainError> {
    match result {
        CommandResult::Unit => Ok(()),
        result => Err(unexpected_result(operation, result)),
    }
}

fn expect_bool(operation: &str, result: CommandResult) -> Result<bool, DomainError> {
    match result {
        CommandResult::Bool(value) => Ok(value),
        result => Err(unexpected_result(operation, result)),
    }
}

fn expect_user(operation: &str, result: CommandResult) -> Result<Option<User>, DomainError> {
    match result {
        CommandResult::User(user) => Ok(*user),
        result => Err(unexpected_result(operation, result)),
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
