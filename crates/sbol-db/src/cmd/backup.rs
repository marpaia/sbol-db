//! Offline complete-backup operations.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use sbol_db_backup::{
    create_complete_backup, generate_x25519_identity_file, parse_x25519_identity,
    verify_encrypted_backup, BackupEncryption, CompleteBackupSource,
};
use sbol_db_rocksdb::Db;
use serde::Serialize;
use uuid::Uuid;

use crate::cli::BackupAction;
use crate::output::print_json;
use crate::runtime::{ManagedDataLayout, LAYOUT_VERSION};

#[derive(Serialize)]
struct GeneratedRecoveryIdentity {
    identity_file: std::path::PathBuf,
    recipient: String,
}

pub fn run(action: BackupAction) -> Result<()> {
    match action {
        BackupAction::Keygen { identity_file } => {
            require_private_parent(&identity_file)?;
            let recipient = generate_x25519_identity_file(&identity_file)?;
            print_json(&GeneratedRecoveryIdentity {
                identity_file,
                recipient: recipient.to_string(),
            })
        }
        BackupAction::Create {
            database_root,
            blobs_root,
            search_root,
            acme_root,
            backup_root,
            identity_file,
        } => {
            require_existing_directory(&database_root, "RocksDB root")?;
            if !database_root.join("CURRENT").is_file() {
                bail!(
                    "RocksDB root has no CURRENT marker: {}",
                    database_root.display()
                );
            }
            require_existing_directory(&blobs_root, "blob root")?;
            require_existing_directory(&search_root, "search root")?;
            require_existing_directory(&acme_root, "ACME root")?;
            verify_private_identity_file(&identity_file)?;
            let identity_contents = fs::read_to_string(&identity_file)
                .with_context(|| format!("read age identity file {}", identity_file.display()))?;
            let identity = parse_x25519_identity(&identity_contents)?;
            let encryption = BackupEncryption::new(identity.to_public(), identity);
            let db = Db::open(&database_root).map_err(anyhow::Error::msg)?;
            let created = create_complete_backup(
                CompleteBackupSource {
                    db: &db,
                    blobs_root: &blobs_root,
                    search_root: &search_root,
                    acme_root: &acme_root,
                    generation: Uuid::new_v4(),
                    layout_version: LAYOUT_VERSION,
                    application_version: env!("CARGO_PKG_VERSION"),
                },
                &backup_root,
                &encryption,
            )?;
            drop(db);
            print_json(&created)
        }
        BackupAction::Verify {
            artifact,
            identity_file,
            staging_dir,
        } => {
            verify_private_identity_file(&identity_file)?;
            let identity_contents = fs::read_to_string(&identity_file)
                .with_context(|| format!("read age identity file {}", identity_file.display()))?;
            let identity = parse_x25519_identity(&identity_contents)?;
            let verified = verify_encrypted_backup(&artifact, &identity, &staging_dir)?;
            print_json(&verified.report())
        }
        BackupAction::Restore {
            artifact,
            identity_file,
            data_dir,
            confirmation,
            remove_artifact_on_success,
            remove_identity_on_success,
        } => {
            require_absolute_data_dir(&data_dir)?;
            verify_private_identity_file(&identity_file)?;
            let identity_contents = fs::read_to_string(&identity_file)
                .with_context(|| format!("read age identity file {}", identity_file.display()))?;
            let identity = parse_x25519_identity(&identity_contents)?;
            let layout = ManagedDataLayout::open(&data_dir)?;
            let verified = verify_encrypted_backup(&artifact, &identity, layout.restore_root())?;
            let restored = layout.restore_verified(verified, &confirmation)?;
            if remove_identity_on_success {
                remove_private_file(&identity_file, "recovery identity")?;
            }
            if remove_artifact_on_success {
                remove_private_file(&artifact, "backup artifact")?;
            }
            print_json(&restored)
        }
        BackupAction::Rollback {
            data_dir,
            confirmation,
        } => {
            require_absolute_data_dir(&data_dir)?;
            let layout = ManagedDataLayout::open(&data_dir)?;
            let rolled_back = layout.rollback(&confirmation)?;
            print_json(&rolled_back)
        }
    }
}

fn require_existing_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "{label} must be a non-symlink directory: {}",
            path.display()
        );
    }
    Ok(())
}

fn require_private_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .context("age identity file has no parent directory")?;
    require_existing_directory(parent, "age identity parent")
}

fn remove_private_file(path: &Path, label: &str) -> Result<()> {
    fs::remove_file(path).with_context(|| format!("remove {label} {}", path.display()))?;
    if let Some(parent) = path.parent() {
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("sync {label} parent {}", parent.display()))?;
    }
    Ok(())
}

fn require_absolute_data_dir(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        bail!("restore --data-dir must be an absolute path");
    }
    Ok(())
}

fn verify_private_identity_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect age identity file {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "age identity must be a regular, non-symlink file: {}",
            path.display()
        );
    }
    verify_private_permissions(path, &metadata)
}

#[cfg(unix)]
fn verify_private_permissions(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if metadata.permissions().mode() & 0o077 != 0 {
        bail!(
            "age identity file must not be accessible by group or others: {}",
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_private_permissions(_path: &Path, _metadata: &fs::Metadata) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn rejects_group_readable_identity_files() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("identity.txt");
        fs::write(&path, "secret").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let error = verify_private_identity_file(&path).unwrap_err().to_string();
        assert!(error.contains("must not be accessible"));
    }
}
