#![allow(clippy::result_large_err)] // OpenRaft's required StorageError carries rich context.

use std::fs::{self, File};
use std::io::{self, Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use openraft::storage::RaftStateMachine;
use openraft::{
    BasicNode, Entry, EntryPayload, LogId, RaftSnapshotBuilder, Snapshot, SnapshotMeta,
    StorageError, StorageIOError, StoredMembership,
};
use rocksdb::WriteBatch;
use sbol_db_core::{ConfigEntry, User, UserId};
use sbol_db_rocksdb::{Db, Durability};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    CommandEnvelope, CommandOutcome, CommandResponse, CommandResult, NodeId, ReplicatedCommand,
    TypeConfig, RAFT_PROTOCOL_VERSION,
};

type StorageResult<T> = Result<T, StorageError<NodeId>>;
type AppliedState = (Option<LogId<NodeId>>, StoredMembership<NodeId, BasicNode>);

const STATE_MACHINE_FORMAT_VERSION: u16 = 1;
const META_COLUMN_FAMILY: &str = "meta";
const CONFIG_COLUMN_FAMILY: &str = "app_config";
const TOKEN_COLUMN_FAMILY: &str = "api_tokens";
const USERS_COLUMN_FAMILY: &str = "users";
const USERS_BY_USERNAME_COLUMN_FAMILY: &str = "users_by_username";
const USERS_BY_EMAIL_COLUMN_FAMILY: &str = "users_by_email";
const LAST_APPLIED_KEY: &[u8] = b"raft/state-machine/last-applied";
const LAST_MEMBERSHIP_KEY: &[u8] = b"raft/state-machine/last-membership";
const CLUSTER_ID_KEY: &[u8] = b"raft/state-machine/cluster-id";
const IDEMPOTENCY_PREFIX: &[u8] = b"raft/state-machine/idempotency/";
const SNAPSHOT_FILE: &str = "current.snapshot";
const SNAPSHOT_MANIFEST: &str = "manifest.json";
const SNAPSHOT_STATE_DIR: &str = "state";
const ACTIVATION_JOURNAL: &str = ".activation.json";
const REPLICATED_COLUMN_FAMILIES: &[&str] = &[
    META_COLUMN_FAMILY,
    CONFIG_COLUMN_FAMILY,
    TOKEN_COLUMN_FAMILY,
    USERS_COLUMN_FAMILY,
    USERS_BY_USERNAME_COLUMN_FAMILY,
    USERS_BY_EMAIL_COLUMN_FAMILY,
];

/// Canonical inventory of one replicated RocksDB column family.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ColumnFamilyAudit {
    pub name: String,
    pub entry_count: usize,
    pub sha256: [u8; 32],
}

/// Deterministic digest of every logical keyspace owned by the Raft state
/// machine. A controller compares this only after a barrier has been applied
/// by all nodes; node-local RocksDB files and paths are intentionally absent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StateAudit {
    pub last_applied: Option<LogId<NodeId>>,
    pub column_families: Vec<ColumnFamilyAudit>,
    pub sha256: [u8; 32],
}

/// A persistent OpenRaft state machine backed by the existing sbol-db RocksDB
/// layout.
///
/// The handle owns the application RocksDB exclusively. Application mutations,
/// the last-applied log id, membership metadata, and idempotency response are
/// committed in one synchronous RocksDB batch.
#[derive(Clone)]
pub struct RocksStateMachine {
    inner: Arc<StateMachineInner>,
}

struct StateMachineInner {
    db: RwLock<Option<Db>>,
    /// Serializes apply, checkpoint construction, and snapshot installation so
    /// snapshot metadata and checkpoint contents always describe one applied
    /// state-machine index.
    mutation: Mutex<()>,
    state_path: PathBuf,
    snapshot_root: PathBuf,
    cluster_id: Uuid,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct IdempotencyRecord {
    request_hash: [u8; 32],
    response: CommandResponse,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct SnapshotManifest {
    format_version: u16,
    protocol_version: u16,
    cluster_id: Uuid,
    meta: SnapshotMeta<NodeId, BasicNode>,
    files: Vec<SnapshotFile>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
struct SnapshotFile {
    path: String,
    size: u64,
    sha256: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct ActivationJournal {
    format_version: u16,
    backup_name: String,
}

impl RocksStateMachine {
    pub fn open(
        state_path: impl AsRef<Path>,
        snapshot_root: impl AsRef<Path>,
        cluster_id: Uuid,
    ) -> StorageResult<Self> {
        let state_path = state_path.as_ref().to_path_buf();
        let snapshot_root = snapshot_root.as_ref().to_path_buf();
        if state_path == snapshot_root
            || state_path.starts_with(&snapshot_root)
            || snapshot_root.starts_with(&state_path)
            || state_path.parent() != snapshot_root.parent()
        {
            return Err(state_write(io::Error::new(
                io::ErrorKind::InvalidInput,
                "state and snapshot directories must be distinct siblings on one filesystem",
            )));
        }

        fs::create_dir_all(&snapshot_root).map_err(state_write)?;
        let pending_backup =
            recover_incomplete_activation(&state_path, &snapshot_root).map_err(state_write)?;
        let db = Db::open_with_durability(&state_path, Durability::Sync).map_err(state_write)?;
        bind_cluster_id(&db, cluster_id)?;
        if let Some(backup) = pending_backup {
            // The new generation was already durably activated before the
            // previous process stopped. Cleanup is deliberately after the
            // candidate has been opened and its cluster identity checked.
            let _ = fs::remove_dir_all(backup);
            let _ = clear_activation_journal(&snapshot_root);
            let _ = sync_parent(&state_path);
        }

        Ok(Self {
            inner: Arc::new(StateMachineInner {
                db: RwLock::new(Some(db)),
                mutation: Mutex::new(()),
                state_path,
                snapshot_root,
                cluster_id,
            }),
        })
    }

    /// Read one configuration entry from the replicated state. This is exposed
    /// for the first leader-read adapter and for recovery tests; mutations must
    /// still enter through Raft.
    pub fn read_config(&self, key: &str) -> StorageResult<Option<ConfigEntry>> {
        self.with_db(|db| {
            db.get_cf(CONFIG_COLUMN_FAMILY, key.as_bytes())
                .map_err(state_read)?
                .map(|bytes| serde_json::from_slice(&bytes).map_err(state_read))
                .transpose()
        })
    }

    pub fn read_all_config(&self) -> StorageResult<Vec<ConfigEntry>> {
        self.with_db(|db| {
            let mut entries = Vec::new();
            db.for_each(CONFIG_COLUMN_FAMILY, |_key, value| {
                entries.push(serde_json::from_slice(value).map_err(|error| {
                    sbol_db_core::DomainError::Database(format!(
                        "config entry is not valid JSON: {error}"
                    ))
                })?);
                Ok(true)
            })
            .map_err(state_read)?;
            entries.sort_by(|left: &ConfigEntry, right: &ConfigEntry| left.key.cmp(&right.key));
            Ok(entries)
        })
    }

    pub fn resolve_token(&self, token_hash: &str) -> StorageResult<Option<UserId>> {
        self.with_db(|db| {
            db.get_cf(TOKEN_COLUMN_FAMILY, token_hash.as_bytes())
                .map_err(state_read)?
                .map(|bytes| Uuid::from_slice(&bytes).map(UserId).map_err(state_read))
                .transpose()
        })
    }

    pub fn find_user(&self, identifier: &str) -> StorageResult<Option<User>> {
        self.with_db(|db| {
            for column_family in [
                USERS_BY_EMAIL_COLUMN_FAMILY,
                USERS_BY_USERNAME_COLUMN_FAMILY,
            ] {
                if let Some(id) = db
                    .get_cf(column_family, identifier.as_bytes())
                    .map_err(state_read)?
                {
                    let id = Uuid::from_slice(&id).map(UserId).map_err(state_read)?;
                    return read_user(db, id);
                }
            }
            Ok(None)
        })
    }

    pub fn read_user(&self, id: UserId) -> StorageResult<Option<User>> {
        self.with_db(|db| read_user(db, id))
    }

    pub fn read_all_users(&self) -> StorageResult<Vec<User>> {
        self.with_db(|db| {
            let mut users = Vec::new();
            db.for_each(USERS_COLUMN_FAMILY, |_key, value| {
                users.push(serde_json::from_slice(value).map_err(|error| {
                    sbol_db_core::DomainError::Database(format!(
                        "user record is not valid JSON: {error}"
                    ))
                })?);
                Ok(true)
            })
            .map_err(state_read)?;
            users.sort_by(|left: &User, right: &User| left.username.cmp(&right.username));
            Ok(users)
        })
    }

    pub fn has_admin(&self) -> StorageResult<bool> {
        self.with_db(|db| {
            let mut found = false;
            db.for_each(USERS_COLUMN_FAMILY, |_key, value| {
                let user: User = serde_json::from_slice(value).map_err(|error| {
                    sbol_db_core::DomainError::Database(format!(
                        "user record is not valid JSON: {error}"
                    ))
                })?;
                found = user.is_admin;
                Ok(!found)
            })
            .map_err(state_read)?;
            Ok(found)
        })
    }

    pub fn audit(&self) -> StorageResult<StateAudit> {
        self.with_db(|db| {
            let mut combined = Sha256::new();
            let mut column_families = Vec::with_capacity(REPLICATED_COLUMN_FAMILIES.len());

            for name in REPLICATED_COLUMN_FAMILIES {
                let mut entries = Vec::<(Vec<u8>, Vec<u8>)>::new();
                db.for_each(name, |key, value| {
                    entries.push((key.to_vec(), value.to_vec()));
                    Ok(true)
                })
                .map_err(state_read)?;
                entries.sort_by(|left, right| left.0.cmp(&right.0));

                let mut digest = Sha256::new();
                hash_part(&mut digest, name.as_bytes());
                for (key, value) in &entries {
                    hash_part(&mut digest, key);
                    hash_part(&mut digest, value);
                }
                let sha256: [u8; 32] = digest.finalize().into();
                hash_part(&mut combined, name.as_bytes());
                hash_part(&mut combined, &sha256);
                column_families.push(ColumnFamilyAudit {
                    name: (*name).to_owned(),
                    entry_count: entries.len(),
                    sha256,
                });
            }

            let last_applied = read_json(db, META_COLUMN_FAMILY, LAST_APPLIED_KEY)?;
            Ok(StateAudit {
                last_applied,
                column_families,
                sha256: combined.finalize().into(),
            })
        })
    }

    fn with_db<T>(&self, operation: impl FnOnce(&Db) -> StorageResult<T>) -> StorageResult<T> {
        let guard = self.inner.db.read().map_err(lock_error)?;
        let db = guard.as_ref().ok_or_else(|| {
            state_read(io::Error::new(
                io::ErrorKind::NotConnected,
                "state machine database is temporarily unavailable",
            ))
        })?;
        operation(db)
    }

    fn applied_state_sync(&self) -> StorageResult<AppliedState> {
        self.with_db(|db| {
            let last_applied = read_json(db, META_COLUMN_FAMILY, LAST_APPLIED_KEY)?;
            let membership = read_json(db, META_COLUMN_FAMILY, LAST_MEMBERSHIP_KEY)?
                .unwrap_or_else(StoredMembership::default);
            Ok((last_applied, membership))
        })
    }

    fn apply_entry(&self, entry: Entry<TypeConfig>) -> StorageResult<CommandResponse> {
        let _mutation = self.inner.mutation.lock().map_err(lock_error)?;
        let log_id = entry.log_id;
        self.with_db(|db| {
            let mut batch = WriteBatch::default();
            batch.put_cf(
                &db.cf(META_COLUMN_FAMILY),
                LAST_APPLIED_KEY,
                serde_json::to_vec(&log_id).map_err(state_write)?,
            );

            let response = match entry.payload {
                EntryPayload::Blank => CommandResponse::internal(log_id.index),
                EntryPayload::Membership(membership) => {
                    let stored = StoredMembership::new(Some(log_id), membership);
                    batch.put_cf(
                        &db.cf(META_COLUMN_FAMILY),
                        LAST_MEMBERSHIP_KEY,
                        serde_json::to_vec(&stored).map_err(state_write)?,
                    );
                    CommandResponse::internal(log_id.index)
                }
                EntryPayload::Normal(request) => {
                    self.stage_command(db, &mut batch, log_id.index, request)?
                }
            };

            db.write(batch)
                .map_err(|error| apply_error(log_id, error))?;
            Ok(response)
        })
    }

    fn stage_command(
        &self,
        db: &Db,
        batch: &mut WriteBatch,
        log_index: u64,
        request: CommandEnvelope,
    ) -> StorageResult<CommandResponse> {
        let idempotency_key = idempotency_key(request.client_id, request.request_id);
        if let Some(existing) =
            read_json::<IdempotencyRecord>(db, META_COLUMN_FAMILY, &idempotency_key)?
        {
            return if existing.request_hash == request.request_hash {
                Ok(existing.response)
            } else {
                Ok(CommandResponse::rejected(
                    request.request_id,
                    log_index,
                    CommandOutcome::IdempotencyConflict,
                ))
            };
        }

        let response = if request.protocol_version != RAFT_PROTOCOL_VERSION {
            CommandResponse::rejected(
                request.request_id,
                log_index,
                CommandOutcome::UnsupportedProtocol {
                    requested: request.protocol_version,
                    supported: RAFT_PROTOCOL_VERSION,
                },
            )
        } else if !request.has_valid_hashes() {
            CommandResponse::rejected(
                request.request_id,
                log_index,
                CommandOutcome::InvalidCommandHash,
            )
        } else {
            let (outcome, result) = match request.command {
                ReplicatedCommand::Barrier => (CommandOutcome::Applied, CommandResult::Unit),
                ReplicatedCommand::SetConfig {
                    key,
                    value,
                    updated_at,
                } => {
                    let entry = ConfigEntry {
                        key,
                        value,
                        updated_at,
                    };
                    batch.put_cf(
                        &db.cf(CONFIG_COLUMN_FAMILY),
                        entry.key.as_bytes(),
                        serde_json::to_vec(&entry).map_err(state_write)?,
                    );
                    (CommandOutcome::Applied, CommandResult::Unit)
                }
                ReplicatedCommand::DeleteConfig { key } => {
                    batch.delete_cf(&db.cf(CONFIG_COLUMN_FAMILY), key.as_bytes());
                    (CommandOutcome::Applied, CommandResult::Unit)
                }
                ReplicatedCommand::IssueToken {
                    token_hash,
                    user_id,
                } => {
                    batch.put_cf(
                        &db.cf(TOKEN_COLUMN_FAMILY),
                        token_hash.as_bytes(),
                        user_id.as_uuid().as_bytes(),
                    );
                    (CommandOutcome::Applied, CommandResult::Unit)
                }
                ReplicatedCommand::RevokeToken { token_hash } => {
                    let existed = db
                        .exists_cf(TOKEN_COLUMN_FAMILY, token_hash.as_bytes())
                        .map_err(state_read)?;
                    if existed {
                        batch.delete_cf(&db.cf(TOKEN_COLUMN_FAMILY), token_hash.as_bytes());
                    }
                    (CommandOutcome::Applied, CommandResult::Bool(existed))
                }
                ReplicatedCommand::CreateUser { user } => {
                    let conflict = if db
                        .exists_cf(USERS_BY_USERNAME_COLUMN_FAMILY, user.username.as_bytes())
                        .map_err(state_read)?
                    {
                        Some(format!("username `{}` already exists", user.username))
                    } else if db
                        .exists_cf(USERS_BY_EMAIL_COLUMN_FAMILY, user.email.as_bytes())
                        .map_err(state_read)?
                    {
                        Some(format!("email `{}` already exists", user.email))
                    } else if db
                        .exists_cf(USERS_COLUMN_FAMILY, user.id.as_uuid().as_bytes())
                        .map_err(state_read)?
                    {
                        Some(format!("user id {} already exists", user.id))
                    } else {
                        None
                    };
                    match conflict {
                        Some(message) => (
                            CommandOutcome::ConstraintViolation { message },
                            CommandResult::Unit,
                        ),
                        None => {
                            stage_put_user(db, batch, &user)?;
                            (
                                CommandOutcome::Applied,
                                CommandResult::User(Box::new(Some(user))),
                            )
                        }
                    }
                }
                ReplicatedCommand::UpdateUserProfile {
                    id,
                    name,
                    affiliation,
                    is_admin,
                    is_curator,
                    is_member,
                    updated_at,
                } => match read_user(db, id)? {
                    Some(mut user) => {
                        user.name = name;
                        user.affiliation = affiliation;
                        user.is_admin = is_admin;
                        user.is_curator = is_curator;
                        user.is_member = is_member;
                        user.updated_at = updated_at;
                        stage_put_user(db, batch, &user)?;
                        (
                            CommandOutcome::Applied,
                            CommandResult::User(Box::new(Some(user))),
                        )
                    }
                    None => (
                        CommandOutcome::NotFound {
                            entity: format!("user {id}"),
                        },
                        CommandResult::Unit,
                    ),
                },
                ReplicatedCommand::SetUserPasswordHash {
                    id,
                    password_hash,
                    updated_at,
                } => match read_user(db, id)? {
                    Some(mut user) => {
                        user.password_hash = password_hash;
                        user.updated_at = updated_at;
                        stage_put_user(db, batch, &user)?;
                        (CommandOutcome::Applied, CommandResult::Unit)
                    }
                    None => (
                        CommandOutcome::NotFound {
                            entity: format!("user {id}"),
                        },
                        CommandResult::Unit,
                    ),
                },
                ReplicatedCommand::SetUserResetLink { id, link } => match read_user(db, id)? {
                    Some(mut user) => {
                        user.reset_password_link = link;
                        stage_put_user(db, batch, &user)?;
                        (CommandOutcome::Applied, CommandResult::Unit)
                    }
                    None => (
                        CommandOutcome::NotFound {
                            entity: format!("user {id}"),
                        },
                        CommandResult::Unit,
                    ),
                },
                ReplicatedCommand::ConsumeUserResetLink { link } => {
                    let mut found = None;
                    db.for_each(USERS_COLUMN_FAMILY, |_key, value| {
                        let user: User = serde_json::from_slice(value).map_err(|error| {
                            sbol_db_core::DomainError::Database(format!(
                                "user record is not valid JSON: {error}"
                            ))
                        })?;
                        if user.reset_password_link.as_deref() == Some(link.as_str()) {
                            found = Some(user);
                            return Ok(false);
                        }
                        Ok(true)
                    })
                    .map_err(state_read)?;
                    if let Some(mut user) = found {
                        user.reset_password_link = None;
                        stage_put_user(db, batch, &user)?;
                        (
                            CommandOutcome::Applied,
                            CommandResult::User(Box::new(Some(user))),
                        )
                    } else {
                        (CommandOutcome::Applied, CommandResult::User(Box::new(None)))
                    }
                }
                ReplicatedCommand::DeleteUser { id } => match read_user(db, id)? {
                    Some(user) => {
                        batch.delete_cf(&db.cf(USERS_COLUMN_FAMILY), user.id.as_uuid().as_bytes());
                        batch.delete_cf(
                            &db.cf(USERS_BY_USERNAME_COLUMN_FAMILY),
                            user.username.as_bytes(),
                        );
                        batch
                            .delete_cf(&db.cf(USERS_BY_EMAIL_COLUMN_FAMILY), user.email.as_bytes());
                        (CommandOutcome::Applied, CommandResult::Bool(true))
                    }
                    None => (CommandOutcome::Applied, CommandResult::Bool(false)),
                },
            };
            match outcome {
                CommandOutcome::Applied => {
                    CommandResponse::applied(request.request_id, log_index, result)
                }
                rejected => CommandResponse::rejected(request.request_id, log_index, rejected),
            }
        };

        let record = IdempotencyRecord {
            request_hash: request.request_hash,
            response: response.clone(),
        };
        batch.put_cf(
            &db.cf(META_COLUMN_FAMILY),
            idempotency_key,
            serde_json::to_vec(&record).map_err(state_write)?,
        );
        Ok(response)
    }

    fn build_snapshot_sync(&self) -> StorageResult<Snapshot<TypeConfig>> {
        let _mutation = self.inner.mutation.lock().map_err(lock_error)?;
        let (last_log_id, last_membership) = self.applied_state_sync()?;
        let snapshot_id = match last_log_id {
            Some(log_id) => format!(
                "{}-{}-{}",
                log_id.leader_id.term,
                log_id.index,
                Uuid::new_v4()
            ),
            None => format!("empty-{}", Uuid::new_v4()),
        };
        let meta = SnapshotMeta {
            last_log_id,
            last_membership,
            snapshot_id,
        };

        let build_root = self
            .inner
            .snapshot_root
            .join(format!(".build-{}", Uuid::new_v4()));
        let checkpoint_path = build_root.join(SNAPSHOT_STATE_DIR);
        fs::create_dir_all(&build_root).map_err(|error| snapshot_write(&meta, error))?;
        let result = (|| {
            self.with_db(|db| {
                db.checkpoint(&checkpoint_path)
                    .map_err(|error| snapshot_write(&meta, error))
            })?;

            let files = collect_snapshot_files(&checkpoint_path)
                .map_err(|error| snapshot_write(&meta, error))?;
            let manifest = SnapshotManifest {
                format_version: STATE_MACHINE_FORMAT_VERSION,
                protocol_version: RAFT_PROTOCOL_VERSION,
                cluster_id: self.inner.cluster_id,
                meta: meta.clone(),
                files,
            };
            let archive = create_snapshot_archive(&build_root, &manifest)
                .map_err(|error| snapshot_write(&meta, error))?;
            persist_current_snapshot(&self.inner.snapshot_root, &archive)
                .map_err(|error| snapshot_write(&meta, error))?;

            Ok(Snapshot {
                meta: meta.clone(),
                snapshot: Box::new(Cursor::new(archive)),
            })
        })();
        let _ = fs::remove_dir_all(&build_root);
        result
    }

    fn install_snapshot_sync(
        &self,
        expected_meta: &SnapshotMeta<NodeId, BasicNode>,
        archive: Vec<u8>,
    ) -> StorageResult<()> {
        let _mutation = self.inner.mutation.lock().map_err(lock_error)?;
        let manifest = read_snapshot_manifest(&archive)
            .map_err(|error| snapshot_read(expected_meta, error))?;
        if manifest.format_version != STATE_MACHINE_FORMAT_VERSION
            || manifest.protocol_version > RAFT_PROTOCOL_VERSION
            || manifest.cluster_id != self.inner.cluster_id
            || manifest.meta != *expected_meta
        {
            return Err(snapshot_read(
                expected_meta,
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "snapshot manifest version or metadata mismatch",
                ),
            ));
        }

        let install_root = self
            .inner
            .snapshot_root
            .join(format!(".install-{}", Uuid::new_v4()));
        fs::create_dir_all(&install_root).map_err(|error| snapshot_write(expected_meta, error))?;
        let result = (|| {
            tar::Archive::new(Cursor::new(&archive))
                .unpack(&install_root)
                .map_err(|error| snapshot_read(expected_meta, error))?;
            let candidate = install_root.join(SNAPSHOT_STATE_DIR);
            verify_snapshot_files(&candidate, &manifest.files)
                .map_err(|error| snapshot_read(expected_meta, error))?;

            // Opening and reading the staged checkpoint detects RocksDB-level
            // corruption before the live generation is touched.
            let staged = Db::open_with_durability(&candidate, Durability::Sync)
                .map_err(|error| snapshot_read(expected_meta, error))?;
            let staged_state = read_applied_state_from_db(&staged)?;
            let staged_cluster = read_json::<Uuid>(&staged, META_COLUMN_FAMILY, CLUSTER_ID_KEY)?;
            drop(staged);
            if staged_cluster != Some(self.inner.cluster_id)
                || staged_state
                    != (
                        expected_meta.last_log_id,
                        expected_meta.last_membership.clone(),
                    )
            {
                return Err(snapshot_read(
                    expected_meta,
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "snapshot RocksDB state does not match its manifest",
                    ),
                ));
            }

            self.activate_checkpoint(&candidate, expected_meta)?;
            persist_current_snapshot(&self.inner.snapshot_root, &archive)
                .map_err(|error| snapshot_write(expected_meta, error))?;
            Ok(())
        })();
        let _ = fs::remove_dir_all(&install_root);
        result
    }

    fn activate_checkpoint(
        &self,
        candidate: &Path,
        meta: &SnapshotMeta<NodeId, BasicNode>,
    ) -> StorageResult<()> {
        let backup = self
            .inner
            .state_path
            .with_file_name(format!(".state-previous-{}", Uuid::new_v4()));
        let failed = self
            .inner
            .state_path
            .with_file_name(format!(".state-failed-{}", Uuid::new_v4()));
        persist_activation_journal(&self.inner.snapshot_root, &backup)
            .map_err(|error| snapshot_write(meta, error))?;
        let mut guard = self.inner.db.write().map_err(lock_error)?;
        let previous = guard.take().ok_or_else(|| {
            snapshot_write(
                meta,
                io::Error::new(io::ErrorKind::NotConnected, "state database is not open"),
            )
        })?;
        drop(previous);

        if let Err(error) = fs::rename(&self.inner.state_path, &backup) {
            *guard = Some(
                Db::open_with_durability(&self.inner.state_path, Durability::Sync)
                    .map_err(|open_error| snapshot_write(meta, open_error))?,
            );
            let _ = clear_activation_journal(&self.inner.snapshot_root);
            return Err(snapshot_write(meta, error));
        }
        if let Err(error) = sync_parent(&self.inner.state_path) {
            let _ = fs::rename(&backup, &self.inner.state_path);
            let _ = sync_parent(&self.inner.state_path);
            *guard = Some(
                Db::open_with_durability(&self.inner.state_path, Durability::Sync)
                    .map_err(|open_error| snapshot_write(meta, open_error))?,
            );
            let _ = clear_activation_journal(&self.inner.snapshot_root);
            return Err(snapshot_write(meta, error));
        }

        if let Err(error) = fs::rename(candidate, &self.inner.state_path) {
            let _ = fs::rename(&backup, &self.inner.state_path);
            let _ = sync_parent(&self.inner.state_path);
            *guard = Some(
                Db::open_with_durability(&self.inner.state_path, Durability::Sync)
                    .map_err(|open_error| snapshot_write(meta, open_error))?,
            );
            let _ = clear_activation_journal(&self.inner.snapshot_root);
            return Err(snapshot_write(meta, error));
        }
        if let Err(error) = sync_parent(&self.inner.state_path) {
            let _ = fs::rename(&self.inner.state_path, &failed);
            let _ = fs::rename(&backup, &self.inner.state_path);
            let _ = sync_parent(&self.inner.state_path);
            *guard = Some(
                Db::open_with_durability(&self.inner.state_path, Durability::Sync)
                    .map_err(|open_error| snapshot_write(meta, open_error))?,
            );
            let _ = clear_activation_journal(&self.inner.snapshot_root);
            return Err(snapshot_write(meta, error));
        }

        match Db::open_with_durability(&self.inner.state_path, Durability::Sync) {
            Ok(db) => {
                *guard = Some(db);
                // The new generation is open and its directory rename was
                // synced. Cleanup failures do not invalidate installation;
                // the journal makes the next startup finish them safely.
                let _ = fs::remove_dir_all(&backup);
                let _ = sync_parent(&self.inner.state_path);
                let _ = clear_activation_journal(&self.inner.snapshot_root);
                Ok(())
            }
            Err(error) => {
                let _ = fs::rename(&self.inner.state_path, &failed);
                let _ = fs::rename(&backup, &self.inner.state_path);
                let _ = sync_parent(&self.inner.state_path);
                *guard = Some(
                    Db::open_with_durability(&self.inner.state_path, Durability::Sync)
                        .map_err(|open_error| snapshot_write(meta, open_error))?,
                );
                let _ = clear_activation_journal(&self.inner.snapshot_root);
                Err(snapshot_write(meta, error))
            }
        }
    }
}

fn hash_part(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
}

impl RaftSnapshotBuilder<TypeConfig> for RocksStateMachine {
    async fn build_snapshot(&mut self) -> StorageResult<Snapshot<TypeConfig>> {
        self.build_snapshot_sync()
    }
}

impl RaftStateMachine<TypeConfig> for RocksStateMachine {
    type SnapshotBuilder = Self;

    async fn applied_state(
        &mut self,
    ) -> StorageResult<(Option<LogId<NodeId>>, StoredMembership<NodeId, BasicNode>)> {
        self.applied_state_sync()
    }

    async fn apply<I>(&mut self, entries: I) -> StorageResult<Vec<CommandResponse>>
    where
        I: IntoIterator<Item = Entry<TypeConfig>> + openraft::OptionalSend,
        I::IntoIter: openraft::OptionalSend,
    {
        entries
            .into_iter()
            .map(|entry| self.apply_entry(entry))
            .collect()
    }

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        self.clone()
    }

    async fn begin_receiving_snapshot(&mut self) -> StorageResult<Box<Cursor<Vec<u8>>>> {
        Ok(Box::new(Cursor::new(Vec::new())))
    }

    async fn install_snapshot(
        &mut self,
        meta: &SnapshotMeta<NodeId, BasicNode>,
        snapshot: Box<Cursor<Vec<u8>>>,
    ) -> StorageResult<()> {
        self.install_snapshot_sync(meta, snapshot.into_inner())
    }

    async fn get_current_snapshot(&mut self) -> StorageResult<Option<Snapshot<TypeConfig>>> {
        let path = self.inner.snapshot_root.join(SNAPSHOT_FILE);
        let archive = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(snapshot_read_without_meta(error)),
        };
        let manifest = read_snapshot_manifest(&archive).map_err(snapshot_read_without_meta)?;
        Ok(Some(Snapshot {
            meta: manifest.meta,
            snapshot: Box::new(Cursor::new(archive)),
        }))
    }
}

fn idempotency_key(client_id: Uuid, request_id: Uuid) -> Vec<u8> {
    let mut key = Vec::with_capacity(IDEMPOTENCY_PREFIX.len() + 32);
    key.extend_from_slice(IDEMPOTENCY_PREFIX);
    key.extend_from_slice(client_id.as_bytes());
    key.extend_from_slice(request_id.as_bytes());
    key
}

fn read_json<T: serde::de::DeserializeOwned>(
    db: &Db,
    column_family: &str,
    key: &[u8],
) -> StorageResult<Option<T>> {
    db.get_cf(column_family, key)
        .map_err(state_read)?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(state_read))
        .transpose()
}

fn read_user(db: &Db, id: UserId) -> StorageResult<Option<User>> {
    db.get_cf(USERS_COLUMN_FAMILY, id.as_uuid().as_bytes())
        .map_err(state_read)?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(state_read))
        .transpose()
}

fn stage_put_user(db: &Db, batch: &mut WriteBatch, user: &User) -> StorageResult<()> {
    let id = user.id.as_uuid().into_bytes();
    batch.put_cf(
        &db.cf(USERS_COLUMN_FAMILY),
        id,
        serde_json::to_vec(user).map_err(state_write)?,
    );
    batch.put_cf(
        &db.cf(USERS_BY_USERNAME_COLUMN_FAMILY),
        user.username.as_bytes(),
        id,
    );
    batch.put_cf(
        &db.cf(USERS_BY_EMAIL_COLUMN_FAMILY),
        user.email.as_bytes(),
        id,
    );
    Ok(())
}

fn read_applied_state_from_db(db: &Db) -> StorageResult<AppliedState> {
    let last_applied = read_json(db, META_COLUMN_FAMILY, LAST_APPLIED_KEY)?;
    let membership = read_json(db, META_COLUMN_FAMILY, LAST_MEMBERSHIP_KEY)?
        .unwrap_or_else(StoredMembership::default);
    Ok((last_applied, membership))
}

fn bind_cluster_id(db: &Db, requested: Uuid) -> StorageResult<()> {
    match read_json::<Uuid>(db, META_COLUMN_FAMILY, CLUSTER_ID_KEY)? {
        Some(stored) if stored != requested => Err(state_write(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("state belongs to cluster {stored}, not cluster {requested}"),
        ))),
        Some(_) => Ok(()),
        None => {
            let mut batch = WriteBatch::default();
            batch.put_cf(
                &db.cf(META_COLUMN_FAMILY),
                CLUSTER_ID_KEY,
                serde_json::to_vec(&requested).map_err(state_write)?,
            );
            db.write(batch).map_err(state_write)
        }
    }
}

fn collect_snapshot_files(root: &Path) -> io::Result<Vec<SnapshotFile>> {
    fn visit(root: &Path, directory: &Path, files: &mut Vec<SnapshotFile>) -> io::Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                visit(root, &path, files)?;
            } else if file_type.is_file() {
                let relative = path.strip_prefix(root).map_err(io::Error::other)?;
                let bytes = fs::read(&path)?;
                files.push(SnapshotFile {
                    path: portable_path(relative)?,
                    size: bytes.len() as u64,
                    sha256: Sha256::digest(&bytes).into(),
                });
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unsupported snapshot file type: {}", path.display()),
                ));
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    visit(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

fn portable_path(path: &Path) -> io::Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "snapshot paths must be relative and normalized",
                ));
            }
        }
    }
    Ok(parts.join("/"))
}

fn create_snapshot_archive(root: &Path, manifest: &SnapshotManifest) -> io::Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    let manifest_bytes = serde_json::to_vec(manifest).map_err(invalid_data)?;
    let mut header = tar::Header::new_gnu();
    header.set_size(manifest_bytes.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    builder.append_data(&mut header, SNAPSHOT_MANIFEST, manifest_bytes.as_slice())?;

    for file in &manifest.files {
        let source = root.join(SNAPSHOT_STATE_DIR).join(&file.path);
        let archive_path = Path::new(SNAPSHOT_STATE_DIR).join(&file.path);
        builder.append_path_with_name(source, archive_path)?;
    }
    builder.into_inner()
}

fn read_snapshot_manifest(archive: &[u8]) -> io::Result<SnapshotManifest> {
    let mut archive = tar::Archive::new(Cursor::new(archive));
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()?.as_ref() == Path::new(SNAPSHOT_MANIFEST) {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            return serde_json::from_slice(&bytes).map_err(invalid_data);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "snapshot manifest is missing",
    ))
}

fn verify_snapshot_files(root: &Path, expected: &[SnapshotFile]) -> io::Result<()> {
    let actual = collect_snapshot_files(root)?;
    if actual != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "snapshot file inventory or checksum mismatch",
        ));
    }
    Ok(())
}

fn persist_current_snapshot(snapshot_root: &Path, archive: &[u8]) -> io::Result<()> {
    fs::create_dir_all(snapshot_root)?;
    let temporary = snapshot_root.join(format!(".current-{}.tmp", Uuid::new_v4()));
    let current = snapshot_root.join(SNAPSHOT_FILE);
    let mut file = File::create(&temporary)?;
    file.write_all(archive)?;
    file.sync_all()?;
    fs::rename(&temporary, &current)?;
    sync_parent(&current)
}

fn persist_activation_journal(snapshot_root: &Path, backup: &Path) -> io::Result<()> {
    let backup_name = backup
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid backup filename"))?;
    validate_single_component(backup_name)?;
    let journal = ActivationJournal {
        format_version: STATE_MACHINE_FORMAT_VERSION,
        backup_name: backup_name.to_owned(),
    };
    let encoded = serde_json::to_vec(&journal).map_err(invalid_data)?;
    let temporary = snapshot_root.join(format!(".activation-{}.tmp", Uuid::new_v4()));
    let current = snapshot_root.join(ACTIVATION_JOURNAL);
    let mut file = File::create(&temporary)?;
    file.write_all(&encoded)?;
    file.sync_all()?;
    fs::rename(&temporary, &current)?;
    sync_parent(&current)
}

/// Recover the only crash-sensitive gap in snapshot generation replacement.
///
/// If the old state was moved aside but the checkpoint was not activated, put
/// the old state back. If both state and backup exist, the checkpoint rename
/// was already made durable; the caller opens and verifies it before removing
/// the backup and journal.
fn recover_incomplete_activation(
    state_path: &Path,
    snapshot_root: &Path,
) -> io::Result<Option<PathBuf>> {
    let journal_path = snapshot_root.join(ACTIVATION_JOURNAL);
    let encoded = match fs::read(&journal_path) {
        Ok(encoded) => encoded,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let journal: ActivationJournal = serde_json::from_slice(&encoded).map_err(invalid_data)?;
    if journal.format_version != STATE_MACHINE_FORMAT_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported snapshot activation journal version",
        ));
    }
    validate_single_component(&journal.backup_name)?;
    let parent = state_path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "state path has no parent"))?;
    let backup = parent.join(&journal.backup_name);

    match (state_path.exists(), backup.exists()) {
        (false, true) => {
            fs::rename(&backup, state_path)?;
            sync_parent(state_path)?;
            clear_activation_journal(snapshot_root)?;
            Ok(None)
        }
        (true, true) => Ok(Some(backup)),
        (true, false) => {
            // The process stopped before moving the old state, or after
            // deleting the backup. In either case the canonical state path is
            // complete; the caller will verify it before normal operation.
            clear_activation_journal(snapshot_root)?;
            Ok(None)
        }
        (false, false) => Err(io::Error::new(
            io::ErrorKind::NotFound,
            "snapshot activation journal exists but both state generations are missing",
        )),
    }
}

fn clear_activation_journal(snapshot_root: &Path) -> io::Result<()> {
    let path = snapshot_root.join(ACTIVATION_JOURNAL);
    match fs::remove_file(&path) {
        Ok(()) => sync_parent(&path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn validate_single_component(component: &str) -> io::Result<()> {
    let mut components = Path::new(component).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(()),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "snapshot activation backup must be one normalized filename",
        )),
    }
}

fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "path has no parent directory")
    })?;
    File::open(parent)?.sync_all()
}

fn state_read(error: impl std::fmt::Display) -> StorageError<NodeId> {
    let error = io::Error::other(error.to_string());
    StorageIOError::read_state_machine(&error).into()
}

fn state_write(error: impl std::fmt::Display) -> StorageError<NodeId> {
    let error = io::Error::other(error.to_string());
    StorageIOError::write_state_machine(&error).into()
}

fn apply_error(log_id: LogId<NodeId>, error: impl std::fmt::Display) -> StorageError<NodeId> {
    let error = io::Error::other(error.to_string());
    StorageIOError::apply(log_id, &error).into()
}

fn snapshot_read(
    meta: &SnapshotMeta<NodeId, BasicNode>,
    error: impl std::fmt::Display,
) -> StorageError<NodeId> {
    let error = io::Error::other(error.to_string());
    StorageIOError::read_snapshot(Some(meta.signature()), &error).into()
}

fn snapshot_read_without_meta(error: impl std::fmt::Display) -> StorageError<NodeId> {
    let error = io::Error::other(error.to_string());
    StorageIOError::read_snapshot(None, &error).into()
}

fn snapshot_write(
    meta: &SnapshotMeta<NodeId, BasicNode>,
    error: impl std::fmt::Display,
) -> StorageError<NodeId> {
    let error = io::Error::other(error.to_string());
    StorageIOError::write_snapshot(Some(meta.signature()), &error).into()
}

fn invalid_data(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn lock_error<T>(error: std::sync::PoisonError<T>) -> StorageError<NodeId> {
    state_read(error)
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use openraft::{CommittedLeaderId, EntryPayload, Membership};
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    fn entry(index: u64, request: CommandEnvelope) -> Entry<TypeConfig> {
        Entry {
            log_id: LogId::new(CommittedLeaderId::new(1, 1), index),
            payload: EntryPayload::Normal(request),
        }
    }

    fn open(root: &Path) -> RocksStateMachine {
        RocksStateMachine::open(
            root.join("state"),
            root.join("snapshots"),
            Uuid::from_u128(1),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn config_apply_and_metadata_survive_restart() {
        let directory = tempdir().unwrap();
        let mut state_machine = open(directory.path());
        let request = CommandEnvelope::new(
            Uuid::from_u128(10),
            Uuid::from_u128(11),
            ReplicatedCommand::SetConfig {
                key: "theme".to_owned(),
                value: json!({"dark": true}),
                updated_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            },
        );

        let response = state_machine.apply([entry(1, request)]).await.unwrap();
        assert_eq!(response[0].outcome, CommandOutcome::Applied);
        drop(state_machine);

        let mut reopened = open(directory.path());
        let config = reopened.read_config("theme").unwrap().unwrap();
        assert_eq!(config.value, json!({"dark": true}));
        assert_eq!(reopened.applied_state().await.unwrap().0.unwrap().index, 1);
    }

    #[tokio::test]
    async fn idempotency_replay_is_exact_and_conflicts_are_rejected() {
        let directory = tempdir().unwrap();
        let mut state_machine = open(directory.path());
        let original = CommandEnvelope::new(
            Uuid::from_u128(20),
            Uuid::from_u128(21),
            ReplicatedCommand::SetConfig {
                key: "mail".to_owned(),
                value: json!({"enabled": true}),
                updated_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            },
        );

        let first = state_machine
            .apply([entry(1, original.clone())])
            .await
            .unwrap();
        let replay = state_machine.apply([entry(2, original)]).await.unwrap();
        assert_eq!(first, replay);

        let new_leader_retry = CommandEnvelope::new(
            Uuid::from_u128(20),
            Uuid::from_u128(21),
            ReplicatedCommand::SetConfig {
                key: "mail".to_owned(),
                value: json!({"enabled": true}),
                updated_at: Utc.timestamp_opt(1_700_000_001, 0).unwrap(),
            },
        );
        let new_leader_retry = state_machine
            .apply([entry(3, new_leader_retry)])
            .await
            .unwrap();
        assert_eq!(first, new_leader_retry);

        let conflict = CommandEnvelope::new(
            Uuid::from_u128(20),
            Uuid::from_u128(21),
            ReplicatedCommand::DeleteConfig {
                key: "mail".to_owned(),
            },
        );
        let conflict = state_machine.apply([entry(4, conflict)]).await.unwrap();
        assert_eq!(conflict[0].outcome, CommandOutcome::IdempotencyConflict);
        assert!(state_machine.read_config("mail").unwrap().is_some());
    }

    #[tokio::test]
    async fn physical_snapshot_replaces_follower_state_after_verification() {
        let leader_dir = tempdir().unwrap();
        let follower_dir = tempdir().unwrap();
        let mut leader = open(leader_dir.path());
        let mut follower = open(follower_dir.path());

        let membership = Membership::new(vec![[1, 2, 3].into_iter().collect()], None);
        leader
            .apply([
                Entry {
                    log_id: LogId::new(CommittedLeaderId::new(1, 1), 1),
                    payload: EntryPayload::Membership(membership),
                },
                entry(
                    2,
                    CommandEnvelope::new(
                        Uuid::from_u128(30),
                        Uuid::from_u128(31),
                        ReplicatedCommand::SetConfig {
                            key: "plugins".to_owned(),
                            value: json!(["search"]),
                            updated_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
                        },
                    ),
                ),
            ])
            .await
            .unwrap();

        let snapshot = leader
            .get_snapshot_builder()
            .await
            .build_snapshot()
            .await
            .unwrap();
        follower
            .install_snapshot(&snapshot.meta, snapshot.snapshot)
            .await
            .unwrap();

        assert_eq!(
            follower.read_config("plugins").unwrap().unwrap().value,
            json!(["search"])
        );
        assert_eq!(
            follower.applied_state().await.unwrap(),
            leader.applied_state().await.unwrap()
        );
        assert!(follower.get_current_snapshot().await.unwrap().is_some());
    }

    #[tokio::test]
    async fn corrupt_snapshot_is_rejected_before_live_state_changes() {
        let leader_dir = tempdir().unwrap();
        let follower_dir = tempdir().unwrap();
        let mut leader = open(leader_dir.path());
        let follower = open(follower_dir.path());

        leader
            .apply([entry(
                1,
                CommandEnvelope::new(
                    Uuid::from_u128(40),
                    Uuid::from_u128(41),
                    ReplicatedCommand::SetConfig {
                        key: "leader".to_owned(),
                        value: json!(true),
                        updated_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
                    },
                ),
            )])
            .await
            .unwrap();
        let snapshot = leader
            .get_snapshot_builder()
            .await
            .build_snapshot()
            .await
            .unwrap();
        let mut archive = snapshot.snapshot.into_inner();
        archive[0] ^= 0xff;

        assert!(follower
            .install_snapshot_sync(&snapshot.meta, archive)
            .is_err());
        assert!(follower.read_config("leader").unwrap().is_none());
        assert_eq!(follower.applied_state_sync().unwrap().0, None);
    }

    #[test]
    fn state_directory_rejects_a_different_cluster() {
        let directory = tempdir().unwrap();
        let state_path = directory.path().join("state");
        let snapshot_path = directory.path().join("snapshots");
        RocksStateMachine::open(&state_path, &snapshot_path, Uuid::from_u128(1)).unwrap();

        let result = RocksStateMachine::open(&state_path, &snapshot_path, Uuid::from_u128(2));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn activation_journal_restores_state_after_the_rename_gap() {
        let directory = tempdir().unwrap();
        let state_path = directory.path().join("state");
        let snapshot_path = directory.path().join("snapshots");
        let mut state =
            RocksStateMachine::open(&state_path, &snapshot_path, Uuid::from_u128(1)).unwrap();
        state
            .apply([entry(
                1,
                CommandEnvelope::new(
                    Uuid::from_u128(50),
                    Uuid::from_u128(51),
                    ReplicatedCommand::SetConfig {
                        key: "survives".to_owned(),
                        value: json!(true),
                        updated_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
                    },
                ),
            )])
            .await
            .unwrap();
        drop(state);

        let backup = directory.path().join(".state-previous-test");
        persist_activation_journal(&snapshot_path, &backup).unwrap();
        fs::rename(&state_path, &backup).unwrap();
        sync_parent(&state_path).unwrap();

        let recovered =
            RocksStateMachine::open(&state_path, &snapshot_path, Uuid::from_u128(1)).unwrap();
        assert_eq!(
            recovered.read_config("survives").unwrap().unwrap().value,
            json!(true)
        );
        assert!(state_path.exists());
        assert!(!backup.exists());
        assert!(!snapshot_path.join(ACTIVATION_JOURNAL).exists());
    }
}
