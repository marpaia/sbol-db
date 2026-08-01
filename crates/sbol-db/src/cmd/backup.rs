//! Offline complete-backup operations.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use sbol_db_backup::{parse_x25519_identity, verify_encrypted_backup};

use crate::cli::BackupAction;
use crate::output::print_json;
use crate::runtime::ManagedDataLayout;

pub fn run(action: BackupAction) -> Result<()> {
    match action {
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
        } => {
            require_absolute_data_dir(&data_dir)?;
            verify_private_identity_file(&identity_file)?;
            let identity_contents = fs::read_to_string(&identity_file)
                .with_context(|| format!("read age identity file {}", identity_file.display()))?;
            let identity = parse_x25519_identity(&identity_contents)?;
            let layout = ManagedDataLayout::open(&data_dir)?;
            let verified = verify_encrypted_backup(&artifact, &identity, layout.restore_root())?;
            let restored = layout.restore_verified(verified, &confirmation)?;
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
