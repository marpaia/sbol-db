use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

use anyhow::{bail, Context, Result};

pub(crate) fn validate_portable_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.starts_with('/')
        || path.ends_with('/')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path.contains('\\')
        || path.bytes().any(|byte| byte == 0)
    {
        bail!("unsafe backup path `{path}`");
    }
    Ok(())
}

pub(crate) fn validate_source_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("{label} must be a real directory: {}", path.display());
    }
    Ok(())
}

pub(crate) fn prepare_private_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            bail!("backup path must be a real directory: {}", path.display())
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path)
                .with_context(|| format!("create backup directory {}", path.display()))?;
        }
        Err(error) => return Err(error.into()),
    }
    set_directory_mode(path, 0o700)
}

pub(crate) fn create_private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    set_open_mode(&mut options, 0o600);
    options
        .open(path)
        .with_context(|| format!("create private backup file {}", path.display()))
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .with_context(|| format!("open directory for sync {}", path.display()))?
        .sync_all()
        .with_context(|| format!("sync directory {}", path.display()))
}

#[cfg(unix)]
pub(crate) fn file_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o777
}

#[cfg(unix)]
pub(crate) fn verify_private_file_mode(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if metadata.permissions().mode() & 0o077 != 0 {
        bail!(
            "backup verification identity must not be accessible by group or others: {}",
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn verify_private_file_mode(_path: &Path, _metadata: &fs::Metadata) -> Result<()> {
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn file_mode(_metadata: &fs::Metadata) -> u32 {
    0o600
}

#[cfg(unix)]
pub(crate) fn set_open_mode(options: &mut OpenOptions, mode: u32) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(mode);
}

#[cfg(not(unix))]
pub(crate) fn set_open_mode(_options: &mut OpenOptions, _mode: u32) {}

#[cfg(unix)]
pub(crate) fn set_file_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777))?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn set_file_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
pub(crate) fn set_directory_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777))?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn set_directory_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}
