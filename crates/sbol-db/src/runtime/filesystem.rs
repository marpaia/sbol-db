use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use sbol_db_rocksdb::Db;
use uuid::Uuid;

use super::layout::LAYOUT_VERSION;

pub(super) fn initialize_layout(
    root: &Path,
    version_path: &Path,
    current_path: &Path,
) -> Result<Uuid> {
    if version_path.exists() {
        validate_layout_version(version_path)?;
    } else {
        atomic_write(root, version_path, format!("{LAYOUT_VERSION}\n").as_bytes())?;
    }

    let generation = Uuid::new_v4();
    let generation_root = root.join("generations").join(generation.to_string());
    prepare_directory(&generation_root.join("rocksdb"), "new RocksDB directory")?;
    prepare_directory(&generation_root.join("blobs"), "new blob root")?;
    prepare_directory(&generation_root.join("search"), "new search directory")?;
    prepare_directory(&generation_root.join("acme"), "new ACME state directory")?;
    atomic_write(root, current_path, format!("{generation}\n").as_bytes())?;
    Ok(generation)
}

pub(super) fn validate_layout_version(path: &Path) -> Result<()> {
    let value = fs::read_to_string(path)
        .with_context(|| format!("reading managed layout version from {}", path.display()))?;
    if value.trim() != LAYOUT_VERSION {
        bail!(
            "unsupported managed data layout version `{}` in {}; expected {LAYOUT_VERSION}",
            value.trim(),
            path.display()
        );
    }
    Ok(())
}

pub(super) fn parse_generation(raw: &str, path: &Path) -> Result<Uuid> {
    let value = raw.trim();
    if value.is_empty() || raw.lines().count() != 1 {
        bail!(
            "{} must contain exactly one generation UUID",
            path.display()
        );
    }
    Uuid::parse_str(value)
        .with_context(|| format!("invalid generation UUID `{value}` in {}", path.display()))
}

pub(super) fn read_generation_pointer(path: &Path, label: &str) -> Result<Uuid> {
    reject_symlink(path, label)?;
    let raw = fs::read_to_string(path)
        .with_context(|| format!("reading {label} from {}", path.display()))?;
    parse_generation(&raw, path)
}

pub(super) fn optional_generation_pointer(path: &Path, label: &str) -> Result<Option<Uuid>> {
    reject_symlink(path, label)?;
    match fs::read_to_string(path) {
        Ok(raw) => parse_generation(&raw, path).map(Some),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {label} {}", path.display())),
    }
}

pub(super) fn read_optional_generation_pointer(path: &Path, label: &str) -> Result<Option<Uuid>> {
    reject_symlink(path, label)?;
    match fs::read_to_string(path) {
        Ok(raw) => parse_generation(&raw, path).map(Some),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("reading {label} from {}", path.display())),
    }
}

pub(super) fn is_pristine_generation(root: &Path) -> Result<bool> {
    require_directory(root, "candidate pristine generation")?;
    for component in ["rocksdb", "blobs", "search", "acme"] {
        let path = root.join(component);
        require_directory(&path, "candidate pristine component")?;
        if fs::read_dir(&path)
            .with_context(|| format!("read candidate pristine component {}", path.display()))?
            .next()
            .transpose()
            .with_context(|| format!("read candidate pristine component {}", path.display()))?
            .is_some()
        {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn verify_generation_structure(root: &Path) -> Result<()> {
    require_directory(root, "retained generation")?;
    for (path, label) in [
        (root.join("rocksdb"), "retained RocksDB directory"),
        (root.join("blobs"), "retained blob root"),
        (root.join("search"), "retained search directory"),
        (root.join("acme"), "retained ACME state directory"),
    ] {
        require_directory(&path, label)?;
    }
    let db = Db::open_read_only(&root.join("rocksdb"))
        .context("open retained RocksDB generation read-only")?;
    drop(db);
    Ok(())
}

pub(super) fn sync_tree(root: &Path) -> Result<()> {
    require_directory(root, "restore payload")?;
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry.with_context(|| format!("read entry below {}", root.display()))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("inspect restore payload path {}", path.display()))?;
        if metadata.file_type().is_symlink() {
            bail!(
                "restore payload must not contain symbolic links: {}",
                path.display()
            );
        }
        if metadata.is_dir() {
            sync_tree(&path)?;
        } else if metadata.is_file() {
            File::open(&path)
                .and_then(|file| file.sync_all())
                .with_context(|| format!("sync restore payload file {}", path.display()))?;
        } else {
            bail!(
                "restore payload contains a non-file entry: {}",
                path.display()
            );
        }
    }
    sync_directory(root)
}

pub(super) fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("sync directory {}", path.display()))
}

pub(super) fn atomic_write(root: &Path, destination: &Path, contents: &[u8]) -> Result<()> {
    let temp = root.join(format!(
        ".{}.{}.tmp",
        destination
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
        Uuid::new_v4().simple()
    ));
    let result = (|| -> Result<()> {
        let mut file = create_private_file(&temp)?;
        file.write_all(contents)
            .with_context(|| format!("writing {}", temp.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing {}", temp.display()))?;
        fs::rename(&temp, destination)
            .with_context(|| format!("atomically replacing {}", destination.display()))?;
        File::open(root)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("syncing data directory {}", root.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub(super) fn reject_symlink(path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("{label} {} must not be a symbolic link", path.display())
        }
        Ok(_) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("inspecting {label} {}", path.display())),
    }
}

pub(super) fn require_directory(path: &Path, label: &str) -> Result<()> {
    reject_symlink(path, label)?;
    let metadata = fs::metadata(path)
        .with_context(|| format!("reading {label} metadata at {}", path.display()))?;
    if !metadata.is_dir() {
        bail!("{label} {} is not a directory", path.display());
    }
    Ok(())
}

pub(super) fn prepare_directory(path: &Path, label: &str) -> Result<()> {
    reject_symlink(path, label)?;
    fs::create_dir_all(path).with_context(|| format!("creating {label} {}", path.display()))?;
    require_directory(path, label)?;
    set_private_directory_permissions(path)?;
    Ok(())
}

pub(super) fn open_private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    set_private_file_mode(&mut options);
    let file = options
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    set_private_file_permissions(path)?;
    Ok(file)
}

pub(super) fn create_private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    set_private_file_mode(&mut options);
    options
        .open(path)
        .with_context(|| format!("creating {}", path.display()))
}

#[cfg(unix)]
pub(super) fn set_private_file_mode(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
pub(super) fn set_private_file_mode(_options: &mut OpenOptions) {}

#[cfg(unix)]
pub(super) fn set_private_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("setting private permissions on {}", path.display()))
}

#[cfg(not(unix))]
pub(super) fn set_private_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
pub(super) fn set_private_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("setting private permissions on {}", path.display()))
}

#[cfg(not(unix))]
pub(super) fn set_private_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}
