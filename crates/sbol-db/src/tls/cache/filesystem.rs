use std::io::{self, Read, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use uuid::Uuid;

use super::MAX_CACHE_ENTRY_BYTES;

pub(super) fn prepare_private_directory(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!(
                "ACME cache directory cannot be a symbolic link: {}",
                path.display()
            )
        }
        Ok(metadata) if !metadata.is_dir() => {
            bail!("ACME cache path is not a directory: {}", path.display())
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path)
                .with_context(|| format!("create ACME cache directory {}", path.display()))?;
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspect ACME cache directory {}", path.display()));
        }
    }
    set_private_directory_permissions(path)?;
    Ok(())
}

pub(super) fn read_private_cache_file(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("ACME cache entry is not a regular file: {}", path.display()),
        ));
    }
    verify_private_file_permissions(path, &metadata)?;
    if metadata.len() > MAX_CACHE_ENTRY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("ACME cache entry exceeds {MAX_CACHE_ENTRY_BYTES} bytes"),
        ));
    }
    let file = std::fs::File::open(path)?;
    let mut contents = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_CACHE_ENTRY_BYTES + 1)
        .read_to_end(&mut contents)?;
    if contents.len() as u64 > MAX_CACHE_ENTRY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("ACME cache entry exceeds {MAX_CACHE_ENTRY_BYTES} bytes"),
        ));
    }
    Ok(Some(contents))
}

pub(super) fn write_private_cache_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    if contents.len() as u64 > MAX_CACHE_ENTRY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("ACME cache entry exceeds {MAX_CACHE_ENTRY_BYTES} bytes"),
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "ACME cache path has no parent")
    })?;
    let temp = parent.join(format!(".acme-cache-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        set_private_open_options(&mut options);
        let mut file = options.open(&temp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        if let Ok(metadata) = std::fs::symlink_metadata(path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "refusing to replace unsafe ACME cache entry: {}",
                        path.display()
                    ),
                ));
            }
        }
        std::fs::rename(&temp, path)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(unix)]
fn set_private_open_options(options: &mut std::fs::OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_private_open_options(_options: &mut std::fs::OpenOptions) {}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("set private permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn verify_private_file_permissions(path: &Path, metadata: &std::fs::Metadata) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "ACME cache entry is accessible by group or others: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_private_file_permissions(_path: &Path, _metadata: &std::fs::Metadata) -> io::Result<()> {
    Ok(())
}
