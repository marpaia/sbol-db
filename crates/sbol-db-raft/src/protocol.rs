use chrono::{DateTime, Utc};
use sbol_db_core::{User, UserId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// The version of the replicated command protocol understood by this binary.
///
/// Every voter must understand a command before it can apply it. Rolling
/// upgrades will therefore gate new command variants on the minimum protocol
/// version reported by the cluster.
pub const RAFT_PROTOCOL_VERSION: u16 = 1;

/// Identity and versioning carried by every client-originated Raft entry.
///
/// `(client_id, request_id)` is the idempotency key. The state machine persists
/// the response with the application mutation, so retrying after a leader
/// change cannot apply the same logical mutation twice.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandEnvelope {
    pub protocol_version: u16,
    pub client_id: Uuid,
    pub request_id: Uuid,
    /// SHA-256 of the serialized logical command, binding an idempotency key to
    /// the exact normalized log payload and detecting transport corruption.
    pub command_hash: [u8; 32],
    /// SHA-256 of client-visible request semantics. Leader-selected fields such
    /// as timestamps are excluded, so a retry handled by a new leader still
    /// returns the first committed result for this idempotency key.
    pub request_hash: [u8; 32],
    pub command: ReplicatedCommand,
}

impl CommandEnvelope {
    pub fn new(client_id: Uuid, request_id: Uuid, command: ReplicatedCommand) -> Self {
        Self {
            protocol_version: RAFT_PROTOCOL_VERSION,
            client_id,
            request_id,
            command_hash: hash_command(&command),
            request_hash: hash_request(&command),
            command,
        }
    }

    pub fn has_valid_hashes(&self) -> bool {
        self.command_hash == hash_command(&self.command)
            && self.request_hash == hash_request(&self.command)
    }
}

/// Version-one replicated operations.
///
/// Commands enter this protocol only when their applied form is deterministic:
/// UUIDs, timestamps, and any derived decisions are chosen by the leader and
/// included in the entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReplicatedCommand {
    /// A committed no-op used as an explicit write/read barrier.
    Barrier,
    /// Upsert durable application configuration. The leader chooses
    /// `updated_at`; followers must never consult their local clocks.
    SetConfig {
        key: String,
        value: Value,
        updated_at: DateTime<Utc>,
    },
    DeleteConfig {
        key: String,
    },
    /// Persist an already-hashed API token. The plaintext token never enters
    /// the replicated log.
    IssueToken {
        token_hash: String,
        user_id: UserId,
    },
    RevokeToken {
        token_hash: String,
    },
    CreateUser {
        user: User,
    },
    UpdateUserProfile {
        id: UserId,
        name: String,
        affiliation: Option<String>,
        is_admin: bool,
        is_curator: bool,
        is_member: bool,
        updated_at: DateTime<Utc>,
    },
    SetUserPasswordHash {
        id: UserId,
        password_hash: String,
        updated_at: DateTime<Utc>,
    },
    SetUserResetLink {
        id: UserId,
        link: Option<String>,
    },
    ConsumeUserResetLink {
        link: String,
    },
    DeleteUser {
        id: UserId,
    },
}

/// Result returned after an entry has been committed and applied locally.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandResponse {
    /// Blank and membership entries are internal and carry no request id.
    pub request_id: Option<Uuid>,
    pub applied_log_index: u64,
    pub outcome: CommandOutcome,
    pub result: CommandResult,
}

impl CommandResponse {
    pub(crate) fn internal(applied_log_index: u64) -> Self {
        Self {
            request_id: None,
            applied_log_index,
            outcome: CommandOutcome::Applied,
            result: CommandResult::Unit,
        }
    }

    pub(crate) fn applied(request_id: Uuid, applied_log_index: u64, result: CommandResult) -> Self {
        Self {
            request_id: Some(request_id),
            applied_log_index,
            outcome: CommandOutcome::Applied,
            result,
        }
    }

    pub(crate) fn rejected(
        request_id: Uuid,
        applied_log_index: u64,
        outcome: CommandOutcome,
    ) -> Self {
        Self {
            request_id: Some(request_id),
            applied_log_index,
            outcome,
            result: CommandResult::Unit,
        }
    }
}

/// Stable domain result for commands whose storage traits return a value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CommandResult {
    Unit,
    Bool(bool),
    User(Box<Option<User>>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CommandOutcome {
    Applied,
    UnsupportedProtocol { requested: u16, supported: u16 },
    InvalidCommandHash,
    IdempotencyConflict,
    ConstraintViolation { message: String },
    NotFound { entity: String },
}

fn hash_command(command: &ReplicatedCommand) -> [u8; 32] {
    canonical_hash(command)
}

fn hash_request(command: &ReplicatedCommand) -> [u8; 32] {
    #[derive(Serialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum LogicalRequest<'a> {
        SetConfig {
            key: &'a str,
            value: &'a Value,
        },
        CreateUser {
            username: &'a str,
            name: &'a str,
            email: &'a str,
            affiliation: &'a Option<String>,
            password_hash: &'a str,
            graph_uri: &'a str,
            is_admin: bool,
            is_curator: bool,
            is_member: bool,
            reset_password_link: &'a Option<String>,
        },
        UpdateUserProfile {
            id: UserId,
            name: &'a str,
            affiliation: &'a Option<String>,
            is_admin: bool,
            is_curator: bool,
            is_member: bool,
        },
        SetUserPasswordHash {
            id: UserId,
            password_hash: &'a str,
        },
        Exact {
            command: &'a ReplicatedCommand,
        },
    }

    let logical = match command {
        ReplicatedCommand::SetConfig { key, value, .. } => LogicalRequest::SetConfig { key, value },
        ReplicatedCommand::CreateUser { user } => LogicalRequest::CreateUser {
            username: &user.username,
            name: &user.name,
            email: &user.email,
            affiliation: &user.affiliation,
            password_hash: &user.password_hash,
            graph_uri: &user.graph_uri,
            is_admin: user.is_admin,
            is_curator: user.is_curator,
            is_member: user.is_member,
            reset_password_link: &user.reset_password_link,
        },
        ReplicatedCommand::UpdateUserProfile {
            id,
            name,
            affiliation,
            is_admin,
            is_curator,
            is_member,
            ..
        } => LogicalRequest::UpdateUserProfile {
            id: *id,
            name,
            affiliation,
            is_admin: *is_admin,
            is_curator: *is_curator,
            is_member: *is_member,
        },
        ReplicatedCommand::SetUserPasswordHash {
            id, password_hash, ..
        } => LogicalRequest::SetUserPasswordHash {
            id: *id,
            password_hash,
        },
        command => LogicalRequest::Exact { command },
    };
    canonical_hash(&logical)
}

/// Hash a canonical JSON tree so object insertion order and serde_json's map
/// implementation are not part of the replicated protocol.
fn canonical_hash(value: &impl Serialize) -> [u8; 32] {
    fn normalize(value: &mut Value) {
        match value {
            Value::Array(values) => values.iter_mut().for_each(normalize),
            Value::Object(map) => {
                let mut entries = std::mem::take(map).into_iter().collect::<Vec<_>>();
                entries.sort_by(|left, right| left.0.cmp(&right.0));
                for (key, mut value) in entries {
                    normalize(&mut value);
                    map.insert(key, value);
                }
            }
            _ => {}
        }
    }

    let mut value =
        serde_json::to_value(value).expect("serializing a Raft command into JSON cannot fail");
    normalize(&mut value);
    let encoded =
        serde_json::to_vec(&value).expect("serializing a canonical JSON value cannot fail");
    Sha256::digest(encoded).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_envelope_has_a_stable_tagged_representation() {
        let command = CommandEnvelope::new(
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            ReplicatedCommand::Barrier,
        );

        let json = serde_json::to_value(&command).unwrap();
        assert_eq!(json["protocol_version"], RAFT_PROTOCOL_VERSION);
        assert_eq!(json["command"]["type"], "barrier");
        assert_eq!(
            serde_json::from_value::<CommandEnvelope>(json).unwrap(),
            command
        );
    }

    #[test]
    fn command_hash_detects_payload_changes() {
        let mut command = CommandEnvelope::new(
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            ReplicatedCommand::DeleteConfig {
                key: "mail".to_owned(),
            },
        );
        assert!(command.has_valid_hashes());

        command.command = ReplicatedCommand::DeleteConfig {
            key: "plugins".to_owned(),
        };
        assert!(!command.has_valid_hashes());
    }

    #[test]
    fn leader_selected_timestamp_is_not_part_of_the_idempotency_hash() {
        let first = CommandEnvelope::new(
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            ReplicatedCommand::SetConfig {
                key: "theme".to_owned(),
                value: serde_json::json!({"dark": true}),
                updated_at: Utc::now(),
            },
        );
        let mut retry = first.clone();
        if let ReplicatedCommand::SetConfig { updated_at, .. } = &mut retry.command {
            *updated_at += chrono::Duration::seconds(1);
        }
        retry.command_hash = hash_command(&retry.command);
        retry.request_hash = hash_request(&retry.command);

        assert_ne!(first.command_hash, retry.command_hash);
        assert_eq!(first.request_hash, retry.request_hash);
        assert!(retry.has_valid_hashes());
    }

    #[test]
    fn json_object_order_is_not_part_of_protocol_hashes() {
        let mut first_value = serde_json::Map::new();
        first_value.insert("alpha".to_owned(), Value::from(1));
        first_value.insert("beta".to_owned(), Value::from(2));
        let mut second_value = serde_json::Map::new();
        second_value.insert("beta".to_owned(), Value::from(2));
        second_value.insert("alpha".to_owned(), Value::from(1));

        let first = CommandEnvelope::new(
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            ReplicatedCommand::SetConfig {
                key: "ordered".to_owned(),
                value: Value::Object(first_value),
                updated_at: Utc::now(),
            },
        );
        let second = CommandEnvelope::new(
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            ReplicatedCommand::SetConfig {
                key: "ordered".to_owned(),
                value: Value::Object(second_value),
                updated_at: match &first.command {
                    ReplicatedCommand::SetConfig { updated_at, .. } => *updated_at,
                    _ => unreachable!(),
                },
            },
        );

        assert_eq!(first.command_hash, second.command_hash);
        assert_eq!(first.request_hash, second.request_hash);
    }
}
