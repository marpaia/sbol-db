use std::path::Path;

use chrono::{DateTime, Utc};
use sbol_db_rocksdb::Db;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const BACKUP_FORMAT: &str = "sbol-db-complete-backup";
pub const BACKUP_VERSION: u32 = 2;
pub(crate) const MANIFEST_PATH: &str = "manifest.json";
pub(crate) const PAYLOAD_PREFIX: &str = "payload";
pub(crate) const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_FILE_COUNT: usize = 10_000_000;
pub(crate) const SBOL2_HASH: &str = "http://sbols.org/v2#hash";
pub(crate) const LEGACY_ATTACHMENT_HASH: &str =
    "http://wiki.synbiohub.org/wiki/Terms/synbiohub#attachmentHash";

/// The four state trees in every complete backup. None is optional, even when
/// a tree is empty, so adding future required state is a format-version change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupComponent {
    Rocksdb,
    Blobs,
    Search,
    Acme,
}

impl BackupComponent {
    pub const ALL: [Self; 4] = [Self::Rocksdb, Self::Blobs, Self::Search, Self::Acme];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rocksdb => "rocksdb",
            Self::Blobs => "blobs",
            Self::Search => "search",
            Self::Acme => "acme",
        }
    }

    pub(crate) fn from_path(path: &str) -> Option<Self> {
        let first = path.split('/').next()?;
        Self::ALL
            .into_iter()
            .find(|component| component.as_str() == first)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupFileManifest {
    /// Slash-separated path relative to the payload root.
    pub path: String,
    pub component: BackupComponent,
    pub size: u64,
    pub sha256: String,
    /// Portable Unix permission bits. Non-Unix creators record 0600.
    pub mode: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupComponentManifest {
    pub component: BackupComponent,
    pub files: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupManifest {
    pub format: String,
    pub version: u32,
    pub backup_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub application_version: String,
    pub layout_version: String,
    pub source_generation: Uuid,
    pub backend: String,
    pub archive: String,
    pub compression: String,
    pub encryption: String,
    pub components: Vec<BackupComponentManifest>,
    pub files: Vec<BackupFileManifest>,
    /// Sorted attachment hashes that the source database references but whose
    /// content was already absent when this backup was created. Authenticating
    /// the exact set preserves a legacy source inconsistency without allowing
    /// additional blob loss during archive creation, transfer, or restore.
    pub missing_referenced_blobs: Vec<String>,
    pub payload_bytes: u64,
}

/// Paths and engine handle captured by one maintenance-barrier-protected
/// backup operation.
pub struct CompleteBackupSource<'a> {
    pub db: &'a Db,
    pub blobs_root: &'a Path,
    pub search_root: &'a Path,
    pub acme_root: &'a Path,
    pub generation: Uuid,
    pub layout_version: &'a str,
    pub application_version: &'a str,
}
