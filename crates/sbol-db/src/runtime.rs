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
use chrono::{DateTime, Utc};
use sbol_db_backup::{verify_payload_directory, VerifiedBackup};
use sbol_db_rocksdb::Db;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::cli::{BackendKind, RuntimeProfile};

const DEFAULT_DATABASE_URL: &str = "postgres://sbol:sbol@localhost:5432/sbol";
const DEFAULT_DEVELOPMENT_DATA_DIR: &str = "sbol-db-data";
const LAYOUT_VERSION: &str = "2";
const VERSION_FILE: &str = "LAYOUT_VERSION";
const CURRENT_FILE: &str = "CURRENT";
const PREVIOUS_FILE: &str = "PREVIOUS";
const LOCK_FILE: &str = "LOCK";
const RESTORE_JOURNAL_FILE: &str = "last-restore.json";

/// Fully resolved storage configuration held for the lifetime of the server.
#[derive(Debug)]
pub struct ServerRuntime {
    profile: RuntimeProfile,
    data_root: PathBuf,
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
                    data_root: layout.root().to_path_buf(),
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
                    data_root: data_dir,
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

    /// Root for process-level durable state. Production components that must
    /// participate in atomic backup/restore use the managed generation paths.
    pub fn data_root(&self) -> &Path {
        &self.data_root
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
enum RestoreJournalStatus {
    Staged,
    Activated,
    RollbackPending,
    RolledBack,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RestoreJournal {
    version: u32,
    status: RestoreJournalStatus,
    backup_id: Uuid,
    artifact_sha256: String,
    previous_generation: Option<Uuid>,
    target_generation: Uuid,
    updated_at: DateTime<Utc>,
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

    /// Materialize a fully verified artifact as a new generation and activate
    /// it with one durable pointer replacement. This method requires the
    /// exclusive layout lock, so it cannot run beside the server.
    pub fn restore_verified(
        &self,
        verified: VerifiedBackup,
        confirmation: &str,
    ) -> Result<RestoreOutcome> {
        let manifest = verified.manifest().clone();
        let artifact_sha256 = verified.artifact_sha256().to_owned();
        if manifest.layout_version != LAYOUT_VERSION {
            bail!(
                "backup layout version `{}` cannot restore into layout version {LAYOUT_VERSION}",
                manifest.layout_version
            );
        }
        let expected_confirmation = format!("RESTORE {}", manifest.backup_id);
        if confirmation != expected_confirmation {
            bail!(
                "restore confirmation mismatch; pass --confirmation {:?}",
                expected_confirmation
            );
        }

        let generations_root = self.root.join("generations");
        let target_root = generations_root.join(manifest.backup_id.to_string());
        if self.generation == manifest.backup_id {
            verify_payload_directory(&manifest, &target_root)
                .context("re-verify already active restored generation")?;
            let previous_generation = read_optional_generation_pointer(
                &self.root.join(PREVIOUS_FILE),
                "previous-generation pointer",
            )?;
            let activated_at = Utc::now();
            self.write_restore_journal(&RestoreJournal {
                version: 1,
                status: RestoreJournalStatus::Activated,
                backup_id: manifest.backup_id,
                artifact_sha256: artifact_sha256.clone(),
                previous_generation,
                target_generation: manifest.backup_id,
                updated_at: activated_at,
            })?;
            return Ok(RestoreOutcome {
                status: "already_active".to_owned(),
                backup_id: manifest.backup_id,
                artifact_sha256,
                previous_generation,
                active_generation: manifest.backup_id,
                activated_at,
                rollback_confirmation: previous_generation
                    .map(|_| format!("ROLLBACK {}", manifest.backup_id)),
            });
        }

        let current_root = generations_root.join(self.generation.to_string());
        let previous_generation = match verify_generation_structure(&current_root) {
            Ok(()) => Some(self.generation),
            Err(_) if is_pristine_generation(&current_root)? => {
                if self.root.join(PREVIOUS_FILE).exists() {
                    bail!(
                        "fresh restore target unexpectedly has a PREVIOUS pointer; refusing to discard recovery state"
                    );
                }
                None
            }
            Err(error) => {
                return Err(error).context(
                    "active generation is neither valid nor pristine; refusing restore without a safe rollback boundary",
                )
            }
        };

        let staging_root =
            generations_root.join(format!(".restore-{}.staging", manifest.backup_id));
        if target_root.exists() {
            verify_payload_directory(&manifest, &target_root)
                .context("verify existing staged restore generation")?;
        } else {
            if staging_root.exists() {
                verify_payload_directory(&manifest, &staging_root)
                    .context("verify resumable restore staging generation")?;
            } else {
                let extracted_root = verified.into_extracted_path();
                let payload_root = extracted_root.join("payload");
                verify_payload_directory(&manifest, &payload_root)
                    .context("verify restore payload before staging")?;
                sync_tree(&payload_root)?;
                fs::rename(&payload_root, &staging_root).with_context(|| {
                    format!(
                        "move verified restore payload {} to {}",
                        payload_root.display(),
                        staging_root.display()
                    )
                })?;
                sync_directory(&generations_root)?;
                fs::remove_dir(&extracted_root).with_context(|| {
                    format!(
                        "remove empty extraction directory {}",
                        extracted_root.display()
                    )
                })?;
                verify_payload_directory(&manifest, &staging_root)
                    .context("verify staged restore generation")?;
            }
            fs::rename(&staging_root, &target_root).with_context(|| {
                format!(
                    "publish restore generation {} as {}",
                    staging_root.display(),
                    target_root.display()
                )
            })?;
            sync_directory(&generations_root)?;
            verify_payload_directory(&manifest, &target_root)
                .context("verify published restore generation")?;
        }

        let activated_at = Utc::now();
        let mut journal = RestoreJournal {
            version: 1,
            status: RestoreJournalStatus::Staged,
            backup_id: manifest.backup_id,
            artifact_sha256: artifact_sha256.clone(),
            previous_generation,
            target_generation: manifest.backup_id,
            updated_at: activated_at,
        };
        self.write_restore_journal(&journal)?;
        if let Some(previous_generation) = previous_generation {
            atomic_write(
                &self.root,
                &self.root.join(PREVIOUS_FILE),
                format!("{previous_generation}\n").as_bytes(),
            )?;
        }
        atomic_write(
            &self.root,
            &self.root.join(CURRENT_FILE),
            format!("{}\n", manifest.backup_id).as_bytes(),
        )?;
        journal.status = RestoreJournalStatus::Activated;
        journal.updated_at = Utc::now();
        self.write_restore_journal(&journal)?;

        Ok(RestoreOutcome {
            status: "activated".to_owned(),
            backup_id: manifest.backup_id,
            artifact_sha256,
            previous_generation,
            active_generation: manifest.backup_id,
            activated_at: journal.updated_at,
            rollback_confirmation: previous_generation
                .map(|_| format!("ROLLBACK {}", manifest.backup_id)),
        })
    }

    /// Atomically switch back to the generation retained by the last restore.
    /// A pending journal is completed idempotently after a crash between the two
    /// pointer writes.
    pub fn rollback(&self, confirmation: &str) -> Result<RollbackOutcome> {
        if let Some(mut pending) = self.read_restore_journal()?.filter(|pending| {
            pending.status == RestoreJournalStatus::RollbackPending
                && pending.previous_generation == Some(self.generation)
        }) {
            let expected = format!("ROLLBACK {}", pending.target_generation);
            if confirmation != expected {
                bail!(
                    "rollback confirmation mismatch; pass --confirmation {:?}",
                    expected
                );
            }
            verify_generation_structure(
                &self
                    .root
                    .join("generations")
                    .join(self.generation.to_string()),
            )?;
            atomic_write(
                &self.root,
                &self.root.join(PREVIOUS_FILE),
                format!("{}\n", pending.target_generation).as_bytes(),
            )?;
            pending.status = RestoreJournalStatus::RolledBack;
            pending.updated_at = Utc::now();
            self.write_restore_journal(&pending)?;
            return Ok(RollbackOutcome {
                status: "resumed".to_owned(),
                previous_generation: pending.target_generation,
                active_generation: self.generation,
                completed_at: pending.updated_at,
                rollback_confirmation: format!("ROLLBACK {}", self.generation),
            });
        }

        let expected = format!("ROLLBACK {}", self.generation);
        if confirmation != expected {
            bail!(
                "rollback confirmation mismatch; pass --confirmation {:?}",
                expected
            );
        }
        let target_generation = read_generation_pointer(
            &self.root.join(PREVIOUS_FILE),
            "previous-generation pointer",
        )?;
        if target_generation == self.generation {
            bail!("previous generation is already active");
        }
        verify_generation_structure(
            &self
                .root
                .join("generations")
                .join(target_generation.to_string()),
        )?;

        let now = Utc::now();
        let prior = self.read_restore_journal()?;
        let mut journal = RestoreJournal {
            version: 1,
            status: RestoreJournalStatus::RollbackPending,
            backup_id: prior
                .as_ref()
                .map(|journal| journal.backup_id)
                .unwrap_or(self.generation),
            artifact_sha256: prior
                .as_ref()
                .map(|journal| journal.artifact_sha256.clone())
                .unwrap_or_default(),
            previous_generation: Some(target_generation),
            target_generation: self.generation,
            updated_at: now,
        };
        self.write_restore_journal(&journal)?;
        atomic_write(
            &self.root,
            &self.root.join(CURRENT_FILE),
            format!("{target_generation}\n").as_bytes(),
        )?;
        atomic_write(
            &self.root,
            &self.root.join(PREVIOUS_FILE),
            format!("{}\n", self.generation).as_bytes(),
        )?;
        journal.status = RestoreJournalStatus::RolledBack;
        journal.updated_at = Utc::now();
        self.write_restore_journal(&journal)?;

        Ok(RollbackOutcome {
            status: "rolled_back".to_owned(),
            previous_generation: self.generation,
            active_generation: target_generation,
            completed_at: journal.updated_at,
            rollback_confirmation: format!("ROLLBACK {target_generation}"),
        })
    }

    fn write_restore_journal(&self, journal: &RestoreJournal) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(journal).context("encode restore journal")?;
        atomic_write(
            &self.restore_root,
            &self.restore_root.join(RESTORE_JOURNAL_FILE),
            &bytes,
        )
    }

    fn read_restore_journal(&self) -> Result<Option<RestoreJournal>> {
        let path = self.restore_root.join(RESTORE_JOURNAL_FILE);
        reject_symlink(&path, "restore journal")?;
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("decode restore journal {}", path.display()))
                .map(Some),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
            Err(err) => {
                Err(err).with_context(|| format!("read restore journal {}", path.display()))
            }
        }
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
    prepare_directory(&generation_root.join("acme"), "new ACME state directory")?;
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

fn read_generation_pointer(path: &Path, label: &str) -> Result<Uuid> {
    reject_symlink(path, label)?;
    let raw = fs::read_to_string(path)
        .with_context(|| format!("reading {label} from {}", path.display()))?;
    parse_generation(&raw, path)
}

fn read_optional_generation_pointer(path: &Path, label: &str) -> Result<Option<Uuid>> {
    reject_symlink(path, label)?;
    match fs::read_to_string(path) {
        Ok(raw) => parse_generation(&raw, path).map(Some),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("reading {label} from {}", path.display())),
    }
}

fn is_pristine_generation(root: &Path) -> Result<bool> {
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

fn verify_generation_structure(root: &Path) -> Result<()> {
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

fn sync_tree(root: &Path) -> Result<()> {
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

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("sync directory {}", path.display()))
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
    use sbol_db_backup::{
        create_complete_backup, verify_encrypted_backup, BackupEncryption, CompleteBackupSource,
    };

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
    fn restores_and_rolls_back_complete_generations() {
        let data = tempfile::tempdir().expect("data tempdir");
        let layout = ManagedDataLayout::open(data.path()).expect("initialize managed layout");
        let original_generation = layout.generation();
        let original_db = Db::open(layout.database_path()).expect("open original database");
        original_db
            .put_cf("meta", b"restore-test", b"original")
            .expect("write original marker");
        drop(original_db);
        fs::write(layout.acme_root().join("account"), b"original-acme")
            .expect("write original ACME state");

        let source = tempfile::tempdir().expect("backup source tempdir");
        for component in ["blobs", "search", "acme", "backups"] {
            fs::create_dir_all(source.path().join(component)).expect("create source component");
        }
        fs::write(source.path().join("search/index"), b"restored-search")
            .expect("write search state");
        fs::write(source.path().join("acme/account"), b"restored-acme").expect("write ACME state");
        let restored_db = Db::open(&source.path().join("rocksdb")).expect("open restored database");
        restored_db
            .put_cf("meta", b"restore-test", b"restored")
            .expect("write restored marker");
        let recovery = age::x25519::Identity::generate();
        let encryption =
            BackupEncryption::new(recovery.to_public(), age::x25519::Identity::generate());
        let created = create_complete_backup(
            CompleteBackupSource {
                db: &restored_db,
                blobs_root: &source.path().join("blobs"),
                search_root: &source.path().join("search"),
                acme_root: &source.path().join("acme"),
                generation: Uuid::new_v4(),
                layout_version: LAYOUT_VERSION,
                application_version: "restore-test",
            },
            &source.path().join("backups"),
            &encryption,
        )
        .expect("create backup");
        drop(restored_db);

        let verified = verify_encrypted_backup(&created.path, &recovery, layout.restore_root())
            .expect("verify backup");
        let restored = layout
            .restore_verified(verified, &format!("RESTORE {}", created.backup_id))
            .expect("activate restored generation");
        assert_eq!(restored.previous_generation, Some(original_generation));
        assert_eq!(restored.active_generation, created.backup_id);
        assert_eq!(
            fs::read_to_string(data.path().join(CURRENT_FILE))
                .expect("read current")
                .trim(),
            created.backup_id.to_string()
        );
        drop(layout);

        let active = ManagedDataLayout::open(data.path()).expect("open restored generation");
        let active_db = Db::open_read_only(active.database_path()).expect("read restored database");
        assert_eq!(
            active_db
                .get_cf("meta", b"restore-test")
                .expect("read restored marker"),
            Some(b"restored".to_vec())
        );
        drop(active_db);
        assert_eq!(
            fs::read(active.acme_root().join("account")).expect("read restored ACME state"),
            b"restored-acme"
        );

        let rolled_back = active
            .rollback(
                restored
                    .rollback_confirmation
                    .as_deref()
                    .expect("rollback is available"),
            )
            .expect("roll back generation");
        assert_eq!(rolled_back.active_generation, original_generation);
        drop(active);

        let original = ManagedDataLayout::open(data.path()).expect("reopen original generation");
        let original_db =
            Db::open_read_only(original.database_path()).expect("read original database");
        assert_eq!(
            original_db
                .get_cf("meta", b"restore-test")
                .expect("read original marker"),
            Some(b"original".to_vec())
        );
        assert_eq!(
            fs::read(original.acme_root().join("account")).expect("read original ACME state"),
            b"original-acme"
        );
        drop(original_db);
        drop(original);

        let fresh_data = tempfile::tempdir().expect("fresh restore tempdir");
        let fresh_layout =
            ManagedDataLayout::open(fresh_data.path()).expect("initialize fresh restore layout");
        let verified =
            verify_encrypted_backup(&created.path, &recovery, fresh_layout.restore_root())
                .expect("verify backup for fresh restore");
        let fresh_restore = fresh_layout
            .restore_verified(verified, &format!("RESTORE {}", created.backup_id))
            .expect("restore into pristine layout");
        assert_eq!(fresh_restore.previous_generation, None);
        assert_eq!(fresh_restore.rollback_confirmation, None);
        drop(fresh_layout);

        let restored_fresh =
            ManagedDataLayout::open(fresh_data.path()).expect("open fresh restored generation");
        let restored_fresh_db = Db::open_read_only(restored_fresh.database_path())
            .expect("read fresh restored database");
        assert_eq!(
            restored_fresh_db
                .get_cf("meta", b"restore-test")
                .expect("read fresh restored marker"),
            Some(b"restored".to_vec())
        );
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
        fs::write(
            temp.path().join(VERSION_FILE),
            format!("{LAYOUT_VERSION}\n"),
        )
        .expect("version");
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
