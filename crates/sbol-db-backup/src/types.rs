use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub struct CreatedBackup {
    pub backup_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub path: PathBuf,
    pub artifact_sha256: String,
    pub artifact_bytes: u64,
    pub payload_bytes: u64,
    pub files: u64,
    pub referenced_blobs: u64,
    pub verified_at: DateTime<Utc>,
    /// True when an at-least-once job retry found and re-verified the artifact
    /// already published for this backup id.
    pub reused: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublishedBackup {
    pub provider: String,
    pub bucket: String,
    pub object_key: String,
    pub artifact_sha256: String,
    pub artifact_bytes: u64,
    pub e_tag: Option<String>,
    pub version: Option<String>,
    pub verified_at: DateTime<Utc>,
    pub reused: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct CompletedBackup {
    #[serde(flatten)]
    pub local: CreatedBackup,
    pub remote: Option<PublishedBackup>,
    pub disk_preflight: BackupDiskPreflight,
    pub local_retention: Option<LocalRetentionReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BackupDiskPreflight {
    pub available_bytes: u64,
    pub estimated_source_bytes: u64,
    pub required_available_bytes: u64,
    pub reserved_bytes: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct LocalRetentionReport {
    pub retained_artifacts: usize,
    pub pruned_artifacts: usize,
    pub pruned_bytes: u64,
}
