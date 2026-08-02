mod recovery;

use std::fs::{self, File};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::filesystem::{
    initialize_layout, open_private_file, parse_generation, prepare_directory, reject_symlink,
    require_directory, validate_layout_version,
};

pub(super) const LAYOUT_VERSION: &str = "2";
pub(super) const VERSION_FILE: &str = "LAYOUT_VERSION";
pub(super) const CURRENT_FILE: &str = "CURRENT";
pub(super) const PREVIOUS_FILE: &str = "PREVIOUS";
pub(super) const LOCK_FILE: &str = "LOCK";
pub(super) const RESTORE_JOURNAL_FILE: &str = "last-restore.json";
pub(super) const RESTORE_HISTORY_DIR: &str = "history";
#[allow(dead_code)] // Consumed once recovery history is exposed by the admin UI layer.
pub(super) const MAX_RECOVERY_HISTORY: usize = 50;

/// Paths for the active production generation. Holding this value also holds
/// the process-wide exclusive data-directory lock.
#[derive(Debug)]
pub struct ManagedDataLayout {
    root: PathBuf,
    generation: Uuid,
    generation_root: PathBuf,
    database_path: PathBuf,
    blob_root: PathBuf,
    search_root: PathBuf,
    acme_root: PathBuf,
    backups_root: PathBuf,
    restore_root: PathBuf,
    _lock: File,
}

#[derive(Clone, Debug, Serialize)]
pub struct RestoreOutcome {
    pub status: String,
    pub backup_id: Uuid,
    pub artifact_sha256: String,
    pub previous_generation: Option<Uuid>,
    pub active_generation: Uuid,
    pub activated_at: DateTime<Utc>,
    pub rollback_confirmation: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RollbackOutcome {
    pub status: String,
    pub previous_generation: Uuid,
    pub active_generation: Uuid,
    pub completed_at: DateTime<Utc>,
    pub rollback_confirmation: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RestoreJournalStatus {
    Staged,
    Activated,
    RollbackPending,
    RolledBack,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryEvent {
    pub version: u32,
    pub status: RestoreJournalStatus,
    pub backup_id: Uuid,
    pub artifact_sha256: String,
    pub previous_generation: Option<Uuid>,
    pub target_generation: Uuid,
    pub updated_at: DateTime<Utc>,
}

type RestoreJournal = RecoveryEvent;

#[derive(Clone, Debug, Serialize)]
#[allow(dead_code)] // Consumed once recovery history is exposed by the admin UI layer.
pub struct RecoveryStatus {
    pub active_generation: Uuid,
    pub previous_generation: Option<Uuid>,
    pub last_operation: Option<RecoveryEvent>,
    pub history: Vec<RecoveryEvent>,
}

impl RestoreJournalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Activated => "activated",
            Self::RollbackPending => "rollback_pending",
            Self::RolledBack => "rolled_back",
        }
    }
}

impl ManagedDataLayout {
    /// Open or initialize a managed data directory and acquire its exclusive
    /// server lock. An existing layout is never silently repaired.
    pub fn open(root: &Path) -> Result<Self> {
        reject_symlink(root, "data directory")?;
        prepare_directory(root, "data directory")?;

        let lock_path = root.join(LOCK_FILE);
        reject_symlink(&lock_path, "data-directory lock")?;
        let lock = open_private_file(&lock_path)?;
        fs2::FileExt::try_lock_exclusive(&lock).with_context(|| {
            format!(
                "locking {}; another sbol-db process may already own this data directory",
                lock_path.display()
            )
        })?;

        let version_path = root.join(VERSION_FILE);
        let current_path = root.join(CURRENT_FILE);
        reject_symlink(&version_path, "layout version file")?;
        reject_symlink(&current_path, "current-generation pointer")?;
        let generation = match fs::read_to_string(&current_path) {
            Ok(raw) => {
                validate_layout_version(&version_path)?;
                parse_generation(&raw, &current_path)?
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                initialize_layout(root, &version_path, &current_path)?
            }
            Err(err) => {
                return Err(err).with_context(|| format!("reading {}", current_path.display()));
            }
        };

        let generations_root = root.join("generations");
        require_directory(&generations_root, "generations directory")?;
        let generation_root = generations_root.join(generation.to_string());
        let database_path = generation_root.join("rocksdb");
        let blob_root = generation_root.join("blobs");
        let search_root = generation_root.join("search");
        let acme_root = generation_root.join("acme");
        for (path, label) in [
            (&generation_root, "active generation"),
            (&database_path, "active RocksDB directory"),
            (&blob_root, "active blob root"),
            (&search_root, "active search directory"),
            (&acme_root, "active ACME state directory"),
        ] {
            require_directory(path, label)?;
        }

        let backups_root = root.join("backups");
        let restore_root = root.join("restore");
        for (path, label) in [
            (&backups_root, "backup staging directory"),
            (&restore_root, "restore staging directory"),
        ] {
            prepare_directory(path, label)?;
        }

        Ok(Self {
            root: root.to_path_buf(),
            generation,
            generation_root,
            database_path,
            blob_root,
            search_root,
            acme_root,
            backups_root,
            restore_root,
            _lock: lock,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn generation(&self) -> Uuid {
        self.generation
    }

    pub fn layout_version(&self) -> &'static str {
        LAYOUT_VERSION
    }

    pub fn generation_root(&self) -> &Path {
        &self.generation_root
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub fn blob_root(&self) -> &Path {
        &self.blob_root
    }

    pub fn search_root(&self) -> &Path {
        &self.search_root
    }

    pub fn acme_root(&self) -> &Path {
        &self.acme_root
    }

    pub fn backups_root(&self) -> &Path {
        &self.backups_root
    }

    pub fn restore_root(&self) -> &Path {
        &self.restore_root
    }
}
