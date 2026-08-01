//! Typed server runtime configuration and the managed production data layout.
//!
//! Production owns one versioned generation below an exclusively locked data
//! directory. Database and blob paths are derived from that generation, so a
//! future restore can stage a complete replacement and activate it by changing
//! one durable pointer instead of mutating an open RocksDB instance.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use uuid::Uuid;

use crate::cli::{BackendKind, RuntimeProfile};

const DEFAULT_DATABASE_URL: &str = "postgres://sbol:sbol@localhost:5432/sbol";
const DEFAULT_DEVELOPMENT_DATA_DIR: &str = "sbol-db-data";
const LAYOUT_VERSION: &str = "1";
const VERSION_FILE: &str = "LAYOUT_VERSION";
const CURRENT_FILE: &str = "CURRENT";
const LOCK_FILE: &str = "LOCK";

/// Fully resolved storage configuration held for the lifetime of the server.
#[derive(Debug)]
pub struct ServerRuntime {
    profile: RuntimeProfile,
    database_url: String,
    blob_root: PathBuf,
    layout: Option<ManagedDataLayout>,
}

impl ServerRuntime {
    /// Resolve the server's database and blob paths. Production is deliberately
    /// closed over one RocksDB topology; development keeps explicit backend
    /// selection available while still using a durable blob directory.
    pub fn resolve(
        profile: RuntimeProfile,
        data_dir: Option<&Path>,
        blob_root: Option<&Path>,
        backend: Option<BackendKind>,
        database_url: Option<&str>,
    ) -> Result<Self> {
        match profile {
            RuntimeProfile::Production => {
                let data_dir = data_dir
                    .context("--data-dir (or SBOL_DB_DATA_DIR) is required in production")?;
                if !data_dir.is_absolute() {
                    bail!("production --data-dir must be an absolute path");
                }
                if let Some(kind) = backend {
                    if kind != BackendKind::Rocksdb {
                        bail!(
                            "production profile manages a RocksDB appliance; \
                             --backend must be rocksdb or omitted"
                        );
                    }
                }
                if database_url.is_some() {
                    bail!(
                        "production profile derives its RocksDB path from --data-dir; \
                         remove --database-url/DATABASE_URL"
                    );
                }
                if blob_root.is_some() {
                    bail!(
                        "production profile derives its blob path from the active generation; \
                         remove --blob-root/SBOL_DB_BLOB_ROOT"
                    );
                }

                let layout = ManagedDataLayout::open(data_dir)?;
                let database_url = format!("rocksdb://{}", layout.database_path().display());
                let blob_root = layout.blob_root().to_path_buf();
                Ok(Self {
                    profile,
                    database_url,
                    blob_root,
                    layout: Some(layout),
                })
            }
            RuntimeProfile::Development => {
                let database_url = resolve_connection(backend, database_url)?;
                let data_dir = data_dir
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_DEVELOPMENT_DATA_DIR));
                let blob_root = blob_root
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| data_dir.join("blobs"));
                prepare_directory(&blob_root, "blob root")?;
                Ok(Self {
                    profile,
                    database_url,
                    blob_root,
                    layout: None,
                })
            }
        }
    }

    pub fn profile(&self) -> RuntimeProfile {
        self.profile
    }

    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    pub fn blob_root(&self) -> &Path {
        &self.blob_root
    }

    pub fn layout(&self) -> Option<&ManagedDataLayout> {
        self.layout.as_ref()
    }
}

/// Resolve an ordinary CLI connection string outside the managed production
/// server. With a backend selector, a bare path gains that backend's scheme and
/// an explicitly conflicting scheme fails closed.
pub fn resolve_connection(backend: Option<BackendKind>, url: Option<&str>) -> Result<String> {
    let url = url.unwrap_or(DEFAULT_DATABASE_URL);
    let Some(backend) = backend else {
        return Ok(url.to_owned());
    };
    match url.split_once("://") {
        Some((scheme, _)) if backend.accepts_scheme(scheme) => Ok(url.to_owned()),
        Some((scheme, _)) => bail!(
            "--backend {} conflicts with --database-url scheme `{scheme}://`; \
             pass a {}:// connection string (or a bare path) or drop --backend",
            backend.scheme(),
            backend.scheme(),
        ),
        None => Ok(format!("{}://{url}", backend.scheme())),
    }
}

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
        for (path, label) in [
            (&generation_root, "active generation"),
            (&database_path, "active RocksDB directory"),
            (&blob_root, "active blob root"),
            (&search_root, "active search directory"),
        ] {
            require_directory(path, label)?;
        }

        let acme_root = root.join("acme");
        let backups_root = root.join("backups");
        let restore_root = root.join("restore");
        for (path, label) in [
            (&acme_root, "ACME state directory"),
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

fn initialize_layout(root: &Path, version_path: &Path, current_path: &Path) -> Result<Uuid> {
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
    atomic_write(root, current_path, format!("{generation}\n").as_bytes())?;
    Ok(generation)
}

fn validate_layout_version(path: &Path) -> Result<()> {
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

fn parse_generation(raw: &str, path: &Path) -> Result<Uuid> {
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

fn atomic_write(root: &Path, destination: &Path, contents: &[u8]) -> Result<()> {
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

fn reject_symlink(path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("{label} {} must not be a symbolic link", path.display())
        }
        Ok(_) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("inspecting {label} {}", path.display())),
    }
}

fn require_directory(path: &Path, label: &str) -> Result<()> {
    reject_symlink(path, label)?;
    let metadata = fs::metadata(path)
        .with_context(|| format!("reading {label} metadata at {}", path.display()))?;
    if !metadata.is_dir() {
        bail!("{label} {} is not a directory", path.display());
    }
    Ok(())
}

fn prepare_directory(path: &Path, label: &str) -> Result<()> {
    reject_symlink(path, label)?;
    fs::create_dir_all(path).with_context(|| format!("creating {label} {}", path.display()))?;
    require_directory(path, label)?;
    set_private_directory_permissions(path)?;
    Ok(())
}

fn open_private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    set_private_file_mode(&mut options);
    let file = options
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    set_private_file_permissions(path)?;
    Ok(file)
}

fn create_private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    set_private_file_mode(&mut options);
    options
        .open(path)
        .with_context(|| format!("creating {}", path.display()))
}

#[cfg(unix)]
fn set_private_file_mode(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_private_file_mode(_options: &mut OpenOptions) {}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("setting private permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("setting private permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes_and_reopens_the_same_generation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("data");
        let first = ManagedDataLayout::open(&root).expect("initialize layout");
        let generation = first.generation();
        assert!(first.database_path().is_dir());
        assert!(first.blob_root().is_dir());
        assert!(first.search_root().is_dir());
        assert!(first.acme_root().is_dir());
        assert!(first.backups_root().is_dir());
        assert!(first.restore_root().is_dir());
        drop(first);

        let reopened = ManagedDataLayout::open(&root).expect("reopen layout");
        assert_eq!(reopened.generation(), generation);
    }

    #[test]
    fn refuses_a_second_owner() {
        let temp = tempfile::tempdir().expect("tempdir");
        let first = ManagedDataLayout::open(temp.path()).expect("first owner");
        let error = ManagedDataLayout::open(temp.path())
            .expect_err("second owner must fail")
            .to_string();
        assert!(error.contains("another sbol-db process"), "got: {error}");
        drop(first);
    }

    #[test]
    fn refuses_a_corrupt_current_pointer() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join(VERSION_FILE), "1\n").expect("version");
        fs::write(temp.path().join(CURRENT_FILE), "../escape\n").expect("current");
        let error = ManagedDataLayout::open(temp.path())
            .expect_err("corrupt pointer must fail")
            .to_string();
        assert!(error.contains("invalid generation UUID"), "got: {error}");
    }

    #[test]
    fn production_derives_all_mutable_paths_from_the_generation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = ServerRuntime::resolve(
            RuntimeProfile::Production,
            Some(temp.path()),
            None,
            None,
            None,
        )
        .expect("production runtime");
        let layout = runtime.layout().expect("managed layout");
        assert_eq!(runtime.blob_root(), layout.generation_root().join("blobs"));
        assert_eq!(
            runtime.database_url(),
            format!(
                "rocksdb://{}",
                layout.generation_root().join("rocksdb").display()
            )
        );
    }

    #[test]
    fn production_rejects_ambiguous_storage_configuration() {
        let temp = tempfile::tempdir().expect("tempdir");
        let relative = ServerRuntime::resolve(
            RuntimeProfile::Production,
            Some(Path::new("relative")),
            None,
            None,
            None,
        )
        .expect_err("relative root must fail")
        .to_string();
        assert!(relative.contains("absolute"), "got: {relative}");

        let explicit_url = ServerRuntime::resolve(
            RuntimeProfile::Production,
            Some(temp.path()),
            None,
            None,
            Some("rocksdb:///other"),
        )
        .expect_err("explicit URL must fail")
        .to_string();
        assert!(explicit_url.contains("remove --database-url"));

        let wrong_backend = ServerRuntime::resolve(
            RuntimeProfile::Production,
            Some(temp.path()),
            None,
            Some(BackendKind::Postgres),
            None,
        )
        .expect_err("wrong backend must fail")
        .to_string();
        assert!(wrong_backend.contains("RocksDB appliance"));
    }

    #[test]
    fn development_defaults_to_postgres_and_durable_blobs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = ServerRuntime::resolve(
            RuntimeProfile::Development,
            Some(temp.path()),
            None,
            None,
            None,
        )
        .expect("development runtime");
        assert_eq!(runtime.database_url(), DEFAULT_DATABASE_URL);
        assert_eq!(runtime.blob_root(), temp.path().join("blobs"));
        assert!(runtime.blob_root().is_dir());
        assert!(runtime.layout().is_none());
    }

    #[test]
    fn connection_selector_preserves_existing_behavior() {
        assert_eq!(
            resolve_connection(Some(BackendKind::Rocksdb), Some("/var/lib/sbol.rocksdb"))
                .expect("bare path"),
            "rocksdb:///var/lib/sbol.rocksdb"
        );
        let error = resolve_connection(
            Some(BackendKind::Sqlite),
            Some("postgres://sbol:sbol@localhost/sbol"),
        )
        .expect_err("conflicting scheme")
        .to_string();
        assert!(error.contains("conflicts"), "got: {error}");
    }
}
