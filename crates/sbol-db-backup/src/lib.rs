//! Complete, encrypted backup artifacts for the single-node RocksDB appliance.
//!
//! The public facade is intentionally small. Format models, key management,
//! archive integrity, object-store publication, and orchestration live in
//! focused modules so each safety boundary can be reviewed independently.

mod archive;
mod encryption;
mod filesystem;
mod manifest;
mod repository;
mod service;
mod types;

pub use archive::{
    create_complete_backup, create_complete_backup_with_id, verify_encrypted_backup,
    verify_payload_directory, VerifiedBackup, VerifiedBackupReport,
};
pub use encryption::{load_or_create_encryption, parse_x25519_identity, BackupEncryption};
pub use manifest::{
    BackupComponent, BackupComponentManifest, BackupFileManifest, BackupManifest,
    CompleteBackupSource, BACKUP_FORMAT, BACKUP_VERSION,
};
pub use repository::{BackupRepository, ObjectStoreBackupRepository};
pub use service::{CompleteBackupConfig, CompleteBackupService};
pub use types::{
    BackupDiskPreflight, CompletedBackup, CreatedBackup, LocalRetentionReport, PublishedBackup,
};

#[cfg(test)]
pub(crate) use manifest::SBOL2_HASH;
#[cfg(test)]
pub(crate) use service::backup_disk_preflight;

#[cfg(test)]
mod tests;
