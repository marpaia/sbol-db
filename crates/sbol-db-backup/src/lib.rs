//! Complete, encrypted backup artifacts for the single-node RocksDB appliance.
//!
//! A backup is one binary age file whose decrypted payload is a zstd-compressed
//! tar stream. It contains a native RocksDB checkpoint plus every filesystem
//! state tree required by the appliance: attachment blobs, the text-search
//! index, and ACME account/certificate material. A versioned manifest records
//! every file's size, mode, and SHA-256 digest.
//!
//! Creation is deliberately blocking and expects the caller to hold the
//! application maintenance barrier introduced by the orchestration layer. The
//! resulting artifact is not returned as verified until it has been read back,
//! decrypted, checksummed entry-by-entry, opened as a read-only RocksDB
//! checkpoint, and checked for every attachment blob referenced by RDF state.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use age::secrecy::ExposeSecret;
use age::x25519;
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use flate2::read::GzDecoder;
use futures::StreamExt;
use object_store::aws::AmazonS3Builder;
use object_store::gcp::GoogleCloudStorageBuilder;
use object_store::path::Path as ObjectPath;
use object_store::{Error as ObjectStoreError, ObjectStore, ObjectStoreExt, WriteMultipart};
use sbol_db_core::ObjectTerm;
use sbol_db_rocksdb::{Db, RocksdbStore};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;
use uuid::Uuid;

pub const BACKUP_FORMAT: &str = "sbol-db-complete-backup";
pub const BACKUP_VERSION: u32 = 1;
const MANIFEST_PATH: &str = "manifest.json";
const PAYLOAD_PREFIX: &str = "payload";
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FILE_COUNT: usize = 10_000_000;
const SBOL2_HASH: &str = "http://sbols.org/v2#hash";
const LEGACY_ATTACHMENT_HASH: &str =
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

    fn from_path(path: &str) -> Option<Self> {
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

/// The configured recovery recipient plus a local identity used to decrypt and
/// verify local and remote readbacks. Artifacts are encrypted to both public
/// keys, so the external recovery identity never needs to live on the server.
#[derive(Clone)]
pub struct BackupEncryption {
    recovery_recipient: x25519::Recipient,
    verification_identity: x25519::Identity,
}

impl BackupEncryption {
    pub fn new(
        recovery_recipient: x25519::Recipient,
        verification_identity: x25519::Identity,
    ) -> Self {
        Self {
            recovery_recipient,
            verification_identity,
        }
    }

    pub fn parse(recovery_recipient: &str, verification_identity: &str) -> Result<Self> {
        let recipient = x25519::Recipient::from_str(recovery_recipient.trim())
            .map_err(|error| anyhow::anyhow!("invalid age recovery recipient: {error}"))?;
        let identity = parse_x25519_identity(verification_identity)?;
        Ok(Self::new(recipient, identity))
    }

    pub fn recovery_recipient(&self) -> &x25519::Recipient {
        &self.recovery_recipient
    }

    pub fn verification_identity(&self) -> &x25519::Identity {
        &self.verification_identity
    }
}

/// Load the server's local verification identity, creating it atomically on
/// first launch. The external recovery recipient is public configuration; its
/// corresponding secret key must be held outside the server.
pub fn load_or_create_encryption(
    recovery_recipient: &str,
    verification_identity_path: &Path,
) -> Result<BackupEncryption> {
    let recovery_recipient = x25519::Recipient::from_str(recovery_recipient.trim())
        .map_err(|error| anyhow::anyhow!("invalid age recovery recipient: {error}"))?;
    let parent = verification_identity_path
        .parent()
        .context("backup verification identity path has no parent")?;
    prepare_private_directory(parent)?;

    let verification_identity = match read_private_identity(verification_identity_path) {
        Ok(identity) => identity,
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|io| io.kind() == io::ErrorKind::NotFound) =>
        {
            create_private_identity(verification_identity_path)?
        }
        Err(error) => return Err(error),
    };
    Ok(BackupEncryption::new(
        recovery_recipient,
        verification_identity,
    ))
}

fn read_private_identity(path: &Path) -> Result<x25519::Identity> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "backup verification identity must be a regular, non-symlink file: {}",
            path.display()
        );
    }
    verify_private_file_mode(path, &metadata)?;
    let contents = fs::read_to_string(path)
        .with_context(|| format!("read backup verification identity {}", path.display()))?;
    parse_x25519_identity(&contents)
}

fn create_private_identity(path: &Path) -> Result<x25519::Identity> {
    let parent = path
        .parent()
        .context("backup verification identity path has no parent")?;
    let identity = x25519::Identity::generate();
    let encoded = identity.to_string();
    let body = format!(
        "# sbol-db local backup verification identity\n# public key: {}\n{}\n",
        identity.to_public(),
        encoded.expose_secret()
    );
    let mut temporary = tempfile::Builder::new()
        .prefix(".backup-verification-identity-")
        .tempfile_in(parent)
        .context("create temporary backup verification identity")?;
    set_file_mode(temporary.path(), 0o600)?;
    temporary
        .write_all(body.as_bytes())
        .context("write backup verification identity")?;
    temporary
        .as_file()
        .sync_all()
        .context("sync backup verification identity")?;
    match temporary.persist_noclobber(path) {
        Ok(_) => {
            sync_directory(parent)?;
            Ok(identity)
        }
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
            read_private_identity(path)
        }
        Err(error) => Err(error.error)
            .with_context(|| format!("publish backup verification identity at {}", path.display())),
    }
}

/// Parse an age secret-key file. Blank lines and age-keygen comments are
/// ignored, but exactly one X25519 identity must remain.
pub fn parse_x25519_identity(contents: &str) -> Result<x25519::Identity> {
    let mut identities = contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'));
    let encoded = identities
        .next()
        .context("age identity file contains no key")?;
    if identities.next().is_some() {
        bail!("age identity file must contain exactly one key");
    }
    x25519::Identity::from_str(encoded)
        .map_err(|error| anyhow::anyhow!("invalid age X25519 identity: {error}"))
}

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

#[async_trait]
pub trait BackupRepository: Send + Sync + 'static {
    async fn publish_verified(
        &self,
        local: &CreatedBackup,
        verification_identity: &x25519::Identity,
        staging_parent: &Path,
    ) -> Result<PublishedBackup>;
}

/// S3/GCS repository backed by Apache Arrow's provider-neutral object-store
/// client. Credentials come only from the providers' standard environment or
/// workload identity; the repository URL contains only bucket and prefix.
pub struct ObjectStoreBackupRepository {
    store: Arc<dyn ObjectStore>,
    provider: String,
    bucket: String,
    prefix: ObjectPath,
}

impl ObjectStoreBackupRepository {
    pub fn from_url(repository_url: &str) -> Result<Self> {
        let url = Url::parse(repository_url).context("parse backup repository URL")?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.port().is_some()
        {
            bail!("backup repository URL may contain only scheme, bucket, and object prefix");
        }
        let bucket = url
            .host_str()
            .filter(|bucket| !bucket.is_empty())
            .context("backup repository URL is missing its bucket")?
            .to_owned();
        let raw_prefix = url.path().trim_matches('/');
        if raw_prefix.is_empty() {
            bail!("backup repository URL must include a non-empty instance prefix");
        }
        let prefix = ObjectPath::from_url_path(raw_prefix)
            .context("backup repository URL has an invalid object prefix")?;
        if prefix.is_root() {
            bail!("backup repository URL must include a non-empty instance prefix");
        }
        let (provider, store): (&str, Arc<dyn ObjectStore>) = match url.scheme() {
            "s3" => {
                if environment_flag("AWS_ALLOW_HTTP") {
                    bail!("AWS_ALLOW_HTTP cannot be enabled for backup repositories");
                }
                let store = AmazonS3Builder::from_env()
                    .with_bucket_name(&bucket)
                    .build()
                    .context("configure S3 backup repository")?;
                ("s3", Arc::new(store))
            }
            "gs" => {
                let store = GoogleCloudStorageBuilder::from_env()
                    .with_bucket_name(&bucket)
                    .build()
                    .context("configure GCS backup repository")?;
                ("gcs", Arc::new(store))
            }
            scheme => {
                bail!("unsupported backup repository scheme `{scheme}`; expected s3:// or gs://")
            }
        };
        Ok(Self::new(store, provider, bucket, prefix))
    }

    pub fn new(
        store: Arc<dyn ObjectStore>,
        provider: impl Into<String>,
        bucket: impl Into<String>,
        prefix: ObjectPath,
    ) -> Self {
        Self {
            store,
            provider: provider.into(),
            bucket: bucket.into(),
            prefix,
        }
    }

    fn object_key(&self, backup: &CreatedBackup) -> Result<ObjectPath> {
        let file_name = backup
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .context("local backup artifact has no UTF-8 filename")?;
        Ok(self
            .prefix
            .clone()
            .join(backup.created_at.format("%Y").to_string())
            .join(backup.created_at.format("%m").to_string())
            .join(backup.created_at.format("%d").to_string())
            .join(file_name))
    }

    async fn verify_remote(
        &self,
        object_key: &ObjectPath,
        local: &CreatedBackup,
        verification_identity: &x25519::Identity,
        staging_parent: &Path,
        reused: bool,
    ) -> Result<PublishedBackup> {
        let result = self
            .store
            .get(object_key)
            .await
            .with_context(|| format!("read back remote backup `{object_key}`"))?;
        if result.meta.size != local.artifact_bytes {
            bail!(
                "remote backup size mismatch for `{object_key}`: local={}, remote={}",
                local.artifact_bytes,
                result.meta.size
            );
        }
        let e_tag = result.meta.e_tag.clone();
        let version = result.meta.version.clone();
        let temporary = tempfile::Builder::new()
            .prefix(".remote-backup-readback-")
            .suffix(".partial")
            .tempfile_in(staging_parent)
            .context("create remote backup readback file")?;
        let mut output = tokio::fs::File::from_std(
            temporary
                .reopen()
                .context("reopen remote backup readback file")?,
        );
        let mut stream = result.into_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| format!("stream remote backup `{object_key}`"))?;
            output
                .write_all(&chunk)
                .await
                .context("write remote backup readback")?;
        }
        output
            .sync_all()
            .await
            .context("sync remote backup readback")?;
        drop(output);
        let verified =
            verify_encrypted_backup(temporary.path(), verification_identity, staging_parent)
                .context("semantic verification of remote backup readback")?;
        if verified.manifest.backup_id != local.backup_id
            || verified.artifact_sha256 != local.artifact_sha256
            || verified.artifact_bytes != local.artifact_bytes
        {
            bail!("remote backup readback does not match the local verified artifact");
        }
        Ok(PublishedBackup {
            provider: self.provider.clone(),
            bucket: self.bucket.clone(),
            object_key: object_key.to_string(),
            artifact_sha256: verified.artifact_sha256,
            artifact_bytes: verified.artifact_bytes,
            e_tag,
            version,
            verified_at: Utc::now(),
            reused,
        })
    }

    async fn upload(&self, object_key: &ObjectPath, local: &CreatedBackup) -> Result<()> {
        let mut input = tokio::fs::File::open(&local.path)
            .await
            .with_context(|| format!("open local backup {}", local.path.display()))?;
        let multipart = self
            .store
            .put_multipart(object_key)
            .await
            .with_context(|| format!("start remote backup upload `{object_key}`"))?;
        let mut upload = WriteMultipart::new(multipart);
        let mut buffer = vec![0_u8; 1024 * 1024];
        loop {
            let read = match input.read(&mut buffer).await {
                Ok(read) => read,
                Err(error) => {
                    let _ = upload.abort().await;
                    return Err(error).context("read local backup for remote upload");
                }
            };
            if read == 0 {
                break;
            }
            if let Err(error) = upload.wait_for_capacity(4).await {
                let _ = upload.abort().await;
                return Err(error).context("wait for remote backup upload capacity");
            }
            upload.write(&buffer[..read]);
        }
        upload
            .finish()
            .await
            .with_context(|| format!("complete remote backup upload `{object_key}`"))?;
        Ok(())
    }
}

#[async_trait]
impl BackupRepository for ObjectStoreBackupRepository {
    async fn publish_verified(
        &self,
        local: &CreatedBackup,
        verification_identity: &x25519::Identity,
        staging_parent: &Path,
    ) -> Result<PublishedBackup> {
        let object_key = self.object_key(local)?;
        match self.store.head(&object_key).await {
            Ok(_) => {
                return self
                    .verify_remote(
                        &object_key,
                        local,
                        verification_identity,
                        staging_parent,
                        true,
                    )
                    .await;
            }
            Err(ObjectStoreError::NotFound { .. }) => {}
            Err(error) => {
                return Err(error).with_context(|| format!("inspect remote backup `{object_key}`"));
            }
        }
        self.upload(&object_key, local).await?;
        self.verify_remote(
            &object_key,
            local,
            verification_identity,
            staging_parent,
            false,
        )
        .await
    }
}

fn environment_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
}

#[derive(Clone)]
pub struct CompleteBackupConfig {
    pub db: Db,
    pub database_root: PathBuf,
    pub blobs_root: PathBuf,
    pub search_root: PathBuf,
    pub acme_root: PathBuf,
    pub backups_root: PathBuf,
    pub generation: Uuid,
    pub layout_version: String,
    pub application_version: String,
    pub minimum_free_bytes: u64,
    pub local_retention: usize,
}

/// Process-local executor for complete backup jobs. Calls are serialized, then
/// run on a blocking thread because checkpointing, hashing, compression, and
/// encryption are synchronous filesystem work.
#[derive(Clone)]
pub struct CompleteBackupService {
    config: Arc<CompleteBackupConfig>,
    encryption: Arc<BackupEncryption>,
    operation: Arc<tokio::sync::Mutex<()>>,
    repository: Option<Arc<dyn BackupRepository>>,
}

impl CompleteBackupService {
    pub fn new(config: CompleteBackupConfig, encryption: BackupEncryption) -> Self {
        Self {
            config: Arc::new(config),
            encryption: Arc::new(encryption),
            operation: Arc::new(tokio::sync::Mutex::new(())),
            repository: None,
        }
    }

    pub fn with_repository(mut self, repository: Arc<dyn BackupRepository>) -> Self {
        self.repository = Some(repository);
        self
    }

    pub async fn create(
        &self,
        backup_id: Uuid,
        requested_at: DateTime<Utc>,
    ) -> Result<CompletedBackup> {
        let _operation = self.operation.lock().await;
        let config = self.config.clone();
        let encryption = self.encryption.clone();
        let (local, disk_preflight) = tokio::task::spawn_blocking(move || {
            let disk_preflight = backup_disk_preflight(&config)?;
            let local = create_complete_backup_with_id(
                CompleteBackupSource {
                    db: &config.db,
                    blobs_root: &config.blobs_root,
                    search_root: &config.search_root,
                    acme_root: &config.acme_root,
                    generation: config.generation,
                    layout_version: &config.layout_version,
                    application_version: &config.application_version,
                },
                &config.backups_root,
                &encryption,
                backup_id,
                requested_at,
            )?;
            Ok::<_, anyhow::Error>((local, disk_preflight))
        })
        .await
        .context("complete backup blocking task panicked")??;
        let (remote, local_retention) = match &self.repository {
            Some(repository) => {
                let published = repository
                    .publish_verified(
                        &local,
                        self.encryption.verification_identity(),
                        &self.config.backups_root,
                    )
                    .await?;
                let artifact = local.path.clone();
                let published_for_retention = published.clone();
                let backup_root = self.config.backups_root.clone();
                let retain = self.config.local_retention;
                let retention = tokio::task::spawn_blocking(move || {
                    record_remote_verification(&artifact, &published_for_retention)?;
                    prune_remotely_verified_local_artifacts(&backup_root, retain)
                })
                .await
                .context("backup retention blocking task panicked")??;
                (Some(published), Some(retention))
            }
            None => (None, None),
        };
        Ok(CompletedBackup {
            local,
            remote,
            disk_preflight,
            local_retention,
        })
    }
}

fn remote_sidecar_path(artifact: &Path) -> PathBuf {
    let name = artifact.file_name().unwrap_or_default().to_string_lossy();
    artifact.with_file_name(format!("{name}.remote.json"))
}

fn record_remote_verification(artifact: &Path, published: &PublishedBackup) -> Result<()> {
    let destination = remote_sidecar_path(artifact);
    let bytes =
        serde_json::to_vec_pretty(published).context("encode remote backup verification")?;
    let mut temporary = tempfile::NamedTempFile::new_in(
        artifact
            .parent()
            .context("backup artifact has no parent directory")?,
    )
    .context("create remote verification sidecar")?;
    temporary
        .write_all(&bytes)
        .context("write remote verification sidecar")?;
    temporary
        .as_file()
        .sync_all()
        .context("sync remote verification sidecar")?;
    set_file_mode(temporary.path(), 0o600)?;
    temporary
        .persist(&destination)
        .map_err(|error| error.error)
        .with_context(|| format!("publish remote verification at {}", destination.display()))?;
    sync_directory(
        artifact
            .parent()
            .context("backup artifact has no parent directory")?,
    )
}

fn prune_remotely_verified_local_artifacts(
    backup_root: &Path,
    retain: usize,
) -> Result<LocalRetentionReport> {
    if retain == 0 {
        bail!("local backup retention must be at least one artifact");
    }
    let mut candidates = Vec::new();
    for entry in fs::read_dir(backup_root)
        .with_context(|| format!("read backup retention directory {}", backup_root.display()))?
    {
        let artifact = entry?.path();
        if artifact
            .extension()
            .is_none_or(|extension| extension != "age")
        {
            continue;
        }
        let metadata = fs::symlink_metadata(&artifact)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        let sidecar = remote_sidecar_path(&artifact);
        let sidecar_metadata = match fs::symlink_metadata(&sidecar) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).context("inspect remote verification sidecar"),
        };
        if sidecar_metadata.len() > MAX_MANIFEST_BYTES {
            bail!(
                "remote verification sidecar is unexpectedly large: {}",
                sidecar.display()
            );
        }
        let published: PublishedBackup = serde_json::from_slice(&fs::read(&sidecar)?)
            .with_context(|| format!("decode remote verification sidecar {}", sidecar.display()))?;
        if sha256_file(&artifact)? != published.artifact_sha256 {
            bail!(
                "local backup no longer matches its remote verification sidecar: {}",
                artifact.display()
            );
        }
        candidates.push((published.verified_at, artifact, sidecar, metadata.len()));
    }
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
    let retained_artifacts = candidates.len().min(retain);
    let mut report = LocalRetentionReport {
        retained_artifacts,
        ..LocalRetentionReport::default()
    };
    for (_, artifact, sidecar, bytes) in candidates.into_iter().skip(retain) {
        // Removing the proof first makes an interrupted prune conservative: a
        // leftover artifact without a sidecar is retained on the next pass.
        fs::remove_file(&sidecar)
            .with_context(|| format!("remove remote verification sidecar {}", sidecar.display()))?;
        fs::remove_file(&artifact)
            .with_context(|| format!("prune local backup artifact {}", artifact.display()))?;
        report.pruned_artifacts += 1;
        report.pruned_bytes = report.pruned_bytes.saturating_add(bytes);
    }
    if report.pruned_artifacts > 0 {
        sync_directory(backup_root)?;
    }
    metrics::gauge!("sbol_db_backup_local_artifacts").set(report.retained_artifacts as f64);
    metrics::counter!("sbol_db_backup_local_pruned_artifacts_total")
        .increment(report.pruned_artifacts as u64);
    metrics::counter!("sbol_db_backup_local_pruned_bytes_total").increment(report.pruned_bytes);
    Ok(report)
}

fn backup_disk_preflight(config: &CompleteBackupConfig) -> Result<BackupDiskPreflight> {
    let estimated_source_bytes = [
        &config.database_root,
        &config.blobs_root,
        &config.search_root,
        &config.acme_root,
    ]
    .into_iter()
    .try_fold(0_u64, |total, path| {
        total
            .checked_add(filesystem_tree_bytes(path)?)
            .context("backup source byte estimate overflow")
    })?;
    // Peak backup work may contain the local artifact, remote readback, and
    // decrypted verification payload simultaneously. Three source-size units
    // plus the operational reserve is deliberately conservative.
    let required_available_bytes = estimated_source_bytes
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(config.minimum_free_bytes))
        .context("backup disk requirement overflow")?;
    let available_bytes = fs2::available_space(&config.backups_root).with_context(|| {
        format!(
            "read available space for backup filesystem {}",
            config.backups_root.display()
        )
    })?;
    metrics::gauge!("sbol_db_backup_preflight_available_bytes").set(available_bytes as f64);
    metrics::gauge!("sbol_db_backup_preflight_required_bytes").set(required_available_bytes as f64);
    metrics::gauge!("sbol_db_backup_estimated_source_bytes").set(estimated_source_bytes as f64);
    if available_bytes < required_available_bytes {
        bail!(
            "insufficient disk space for complete backup: available={available_bytes}, required={required_available_bytes}, estimated_source={estimated_source_bytes}, reserve={}",
            config.minimum_free_bytes
        );
    }
    Ok(BackupDiskPreflight {
        available_bytes,
        estimated_source_bytes,
        required_available_bytes,
        reserved_bytes: config.minimum_free_bytes,
    })
}

fn filesystem_tree_bytes(root: &Path) -> Result<u64> {
    validate_source_directory(root, "backup size source")?;
    let mut total = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .with_context(|| format!("read backup size source {}", directory.display()))?
        {
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)
                .with_context(|| format!("inspect backup size source {}", path.display()))?;
            if metadata.file_type().is_symlink() {
                bail!(
                    "backup size source contains a symbolic link: {}",
                    path.display()
                );
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                total = total
                    .checked_add(metadata.len())
                    .context("backup source byte estimate overflow")?;
            } else {
                bail!(
                    "backup size source contains a special file: {}",
                    path.display()
                );
            }
        }
    }
    Ok(total)
}

#[derive(Clone, Debug, Serialize)]
pub struct VerifiedBackupReport {
    pub status: &'static str,
    pub backup_id: Uuid,
    pub restore_confirmation: String,
    pub created_at: DateTime<Utc>,
    pub application_version: String,
    pub layout_version: String,
    pub source_generation: Uuid,
    pub artifact_sha256: String,
    pub artifact_bytes: u64,
    pub payload_bytes: u64,
    pub files: u64,
    pub referenced_blobs: u64,
    pub components: Vec<BackupComponentManifest>,
}

/// A decrypted backup that remains extracted until dropped. Restore can stage
/// this payload directly and atomically activate it in a later phase.
pub struct VerifiedBackup {
    manifest: BackupManifest,
    artifact_sha256: String,
    artifact_bytes: u64,
    referenced_blobs: u64,
    extracted: TempDir,
}

impl VerifiedBackup {
    pub fn manifest(&self) -> &BackupManifest {
        &self.manifest
    }

    pub fn artifact_sha256(&self) -> &str {
        &self.artifact_sha256
    }

    pub fn artifact_bytes(&self) -> u64 {
        self.artifact_bytes
    }

    pub fn referenced_blobs(&self) -> u64 {
        self.referenced_blobs
    }

    pub fn payload_root(&self) -> PathBuf {
        self.extracted.path().join(PAYLOAD_PREFIX)
    }

    pub fn report(&self) -> VerifiedBackupReport {
        VerifiedBackupReport {
            status: "verified",
            backup_id: self.manifest.backup_id,
            restore_confirmation: format!("RESTORE {}", self.manifest.backup_id),
            created_at: self.manifest.created_at,
            application_version: self.manifest.application_version.clone(),
            layout_version: self.manifest.layout_version.clone(),
            source_generation: self.manifest.source_generation,
            artifact_sha256: self.artifact_sha256.clone(),
            artifact_bytes: self.artifact_bytes,
            payload_bytes: self.manifest.payload_bytes,
            files: self.manifest.files.len() as u64,
            referenced_blobs: self.referenced_blobs,
            components: self.manifest.components.clone(),
        }
    }

    /// Keep the verified extraction directory for staged restore.
    pub fn into_extracted_path(self) -> PathBuf {
        self.extracted.keep()
    }
}

/// Create, encrypt, and read-back-verify one complete local artifact. The final
/// name appears atomically only after verification succeeds.
pub fn create_complete_backup(
    source: CompleteBackupSource<'_>,
    backup_root: &Path,
    encryption: &BackupEncryption,
) -> Result<CreatedBackup> {
    create_complete_backup_with_id(source, backup_root, encryption, Uuid::new_v4(), Utc::now())
}

/// Create a backup with caller-supplied identity and request time. Durable job
/// runners use the job UUID here, making retries idempotent across crashes.
pub fn create_complete_backup_with_id(
    source: CompleteBackupSource<'_>,
    backup_root: &Path,
    encryption: &BackupEncryption,
    backup_id: Uuid,
    created_at: DateTime<Utc>,
) -> Result<CreatedBackup> {
    prepare_private_directory(backup_root)?;
    let final_path = backup_path(backup_root, backup_id, created_at);
    if final_path.exists() {
        let verified =
            verify_encrypted_backup(&final_path, encryption.verification_identity(), backup_root)
                .context("verify artifact published by an earlier backup attempt")?;
        if verified.manifest.backup_id != backup_id || verified.manifest.created_at != created_at {
            bail!("existing backup artifact does not match the requested backup identity");
        }
        return Ok(created_from_verified(final_path, verified, true));
    }
    validate_source_directory(source.blobs_root, "blob root")?;
    validate_source_directory(source.search_root, "search root")?;
    validate_source_directory(source.acme_root, "ACME root")?;

    let staging = tempfile::Builder::new()
        .prefix(&format!(".backup-stage-{backup_id}-"))
        .tempdir_in(backup_root)
        .context("create backup staging directory")?;
    let payload_root = staging.path().join(PAYLOAD_PREFIX);
    prepare_private_directory(&payload_root)?;
    for component in BackupComponent::ALL
        .into_iter()
        .filter(|component| *component != BackupComponent::Rocksdb)
    {
        prepare_private_directory(&payload_root.join(component.as_str()))?;
    }

    source
        .db
        .create_checkpoint(&payload_root.join(BackupComponent::Rocksdb.as_str()))
        .context("create native RocksDB checkpoint")?;
    copy_tree(
        source.blobs_root,
        &payload_root.join(BackupComponent::Blobs.as_str()),
    )?;
    copy_tree(
        source.search_root,
        &payload_root.join(BackupComponent::Search.as_str()),
    )?;
    copy_tree(
        source.acme_root,
        &payload_root.join(BackupComponent::Acme.as_str()),
    )?;

    let files = collect_payload_files(&payload_root)?;
    let payload_bytes = files.iter().try_fold(0_u64, |sum, file| {
        sum.checked_add(file.size)
            .context("backup payload byte count overflow")
    })?;
    let components = BackupComponent::ALL
        .into_iter()
        .map(|component| BackupComponentManifest {
            component,
            files: files
                .iter()
                .filter(|file| file.component == component)
                .count() as u64,
            bytes: files
                .iter()
                .filter(|file| file.component == component)
                .map(|file| file.size)
                .sum(),
        })
        .collect();
    let manifest = BackupManifest {
        format: BACKUP_FORMAT.to_owned(),
        version: BACKUP_VERSION,
        backup_id,
        created_at,
        application_version: source.application_version.to_owned(),
        layout_version: source.layout_version.to_owned(),
        source_generation: source.generation,
        backend: "rocksdb".to_owned(),
        archive: "tar".to_owned(),
        compression: "zstd".to_owned(),
        encryption: "age-x25519".to_owned(),
        components,
        files,
        payload_bytes,
    };
    validate_manifest(&manifest)?;

    let mut partial = tempfile::Builder::new()
        .prefix(&format!(".backup-{backup_id}-"))
        .suffix(".partial")
        .tempfile_in(backup_root)
        .context("create partial backup artifact")?;
    write_encrypted_archive(partial.as_file_mut(), &payload_root, &manifest, encryption)?;
    partial
        .as_file()
        .sync_all()
        .context("sync encrypted backup artifact")?;

    let verified = verify_encrypted_backup(
        partial.path(),
        encryption.verification_identity(),
        backup_root,
    )
    .context("read-back verification of new backup")?;
    if verified.manifest.backup_id != backup_id {
        bail!("read-back manifest backup id changed unexpectedly");
    }
    partial
        .persist_noclobber(&final_path)
        .map_err(|error| error.error)
        .with_context(|| format!("publish verified backup at {}", final_path.display()))?;
    sync_directory(backup_root)?;

    Ok(created_from_verified(final_path, verified, false))
}

fn backup_path(backup_root: &Path, backup_id: Uuid, created_at: DateTime<Utc>) -> PathBuf {
    let stamp = created_at
        .to_rfc3339_opts(SecondsFormat::Secs, true)
        .replace([':', '-'], "");
    backup_root.join(format!("sbol-db-{stamp}-{backup_id}.sbolbackup.age"))
}

fn created_from_verified(path: PathBuf, verified: VerifiedBackup, reused: bool) -> CreatedBackup {
    CreatedBackup {
        backup_id: verified.manifest.backup_id,
        created_at: verified.manifest.created_at,
        path,
        artifact_sha256: verified.artifact_sha256,
        artifact_bytes: verified.artifact_bytes,
        payload_bytes: verified.manifest.payload_bytes,
        files: verified.manifest.files.len() as u64,
        referenced_blobs: verified.referenced_blobs,
        verified_at: Utc::now(),
        reused,
    }
}

/// Decrypt and validate an artifact into a private temporary directory. This is
/// the same verifier used before local publication and later object-store
/// readback and restore.
pub fn verify_encrypted_backup(
    artifact: &Path,
    identity: &dyn age::Identity,
    staging_parent: &Path,
) -> Result<VerifiedBackup> {
    prepare_private_directory(staging_parent)?;
    let artifact_metadata = fs::symlink_metadata(artifact)
        .with_context(|| format!("inspect backup artifact {}", artifact.display()))?;
    if artifact_metadata.file_type().is_symlink() || !artifact_metadata.is_file() {
        bail!(
            "backup artifact must be a regular, non-symlink file: {}",
            artifact.display()
        );
    }
    let artifact_bytes = artifact_metadata.len();
    let artifact_sha256 = sha256_file(artifact)?;
    let input = BufReader::new(
        File::open(artifact)
            .with_context(|| format!("open encrypted backup {}", artifact.display()))?,
    );
    let decryptor = age::Decryptor::new(input).context("parse age backup envelope")?;
    let decrypted = decryptor
        .decrypt(std::iter::once(identity))
        .context("decrypt backup artifact")?;
    let decompressed =
        zstd::stream::read::Decoder::new(decrypted).context("open zstd backup payload")?;
    let mut archive = tar::Archive::new(decompressed);
    let mut entries = archive.entries().context("read backup tar entries")?;

    let mut manifest_entry = entries
        .next()
        .context("backup tar is empty")?
        .context("read backup manifest entry")?;
    let manifest_path = normalized_tar_path(&manifest_entry)?;
    if manifest_path != MANIFEST_PATH || !manifest_entry.header().entry_type().is_file() {
        bail!("backup manifest must be the first regular tar entry");
    }
    if manifest_entry.size() > MAX_MANIFEST_BYTES {
        bail!("backup manifest exceeds {MAX_MANIFEST_BYTES} bytes");
    }
    let mut manifest_bytes = Vec::with_capacity(manifest_entry.size() as usize);
    manifest_entry
        .by_ref()
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut manifest_bytes)
        .context("read backup manifest")?;
    if manifest_bytes.len() as u64 > MAX_MANIFEST_BYTES {
        bail!("backup manifest exceeds {MAX_MANIFEST_BYTES} bytes");
    }
    let manifest: BackupManifest =
        serde_json::from_slice(&manifest_bytes).context("decode backup manifest")?;
    validate_manifest(&manifest)?;

    let extracted = tempfile::Builder::new()
        .prefix(&format!(".backup-verify-{}-", manifest.backup_id))
        .tempdir_in(staging_parent)
        .context("create backup verification directory")?;
    let payload_root = extracted.path().join(PAYLOAD_PREFIX);
    prepare_private_directory(&payload_root)?;
    for component in BackupComponent::ALL {
        prepare_private_directory(&payload_root.join(component.as_str()))?;
    }

    let mut expected: BTreeMap<_, _> = manifest
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    for entry in entries {
        let mut entry = entry.context("read backup tar entry")?;
        if !entry.header().entry_type().is_file() {
            bail!("backup payload may contain only regular files");
        }
        let path = normalized_tar_path(&entry)?;
        let relative = path
            .strip_prefix(&format!("{PAYLOAD_PREFIX}/"))
            .context("backup tar entry is outside payload/")?;
        let expected_file = expected
            .remove(relative)
            .with_context(|| format!("unexpected or duplicate backup entry `{relative}`"))?;
        if entry.size() != expected_file.size {
            bail!(
                "backup entry size mismatch for `{relative}`: manifest={}, archive={}",
                expected_file.size,
                entry.size()
            );
        }
        let destination = payload_root.join(portable_to_path(relative)?);
        let parent = destination
            .parent()
            .context("backup entry has no parent directory")?;
        prepare_private_directory(parent)?;
        let mut output = create_private_file(&destination)?;
        let mut hasher = Sha256::new();
        let mut copied = 0_u64;
        let mut buffer = [0_u8; 128 * 1024];
        loop {
            let read = entry
                .read(&mut buffer)
                .context("read backup payload entry")?;
            if read == 0 {
                break;
            }
            copied = copied
                .checked_add(read as u64)
                .context("backup entry byte count overflow")?;
            if copied > expected_file.size {
                bail!("backup entry exceeds declared size for `{relative}`");
            }
            output
                .write_all(&buffer[..read])
                .with_context(|| format!("write extracted backup entry `{relative}`"))?;
            hasher.update(&buffer[..read]);
        }
        if copied != expected_file.size {
            bail!("backup entry ended early for `{relative}`");
        }
        output.sync_all()?;
        set_file_mode(&destination, expected_file.mode)?;
        let actual = hex::encode(hasher.finalize());
        if actual != expected_file.sha256 {
            bail!("backup entry checksum mismatch for `{relative}`");
        }
    }
    if !expected.is_empty() {
        let missing: Vec<_> = expected.keys().copied().take(10).collect();
        bail!("backup is missing manifest entries: {missing:?}");
    }

    let referenced_blobs = verify_payload_directory(&manifest, &payload_root)?;

    Ok(VerifiedBackup {
        manifest,
        artifact_sha256,
        artifact_bytes,
        referenced_blobs: referenced_blobs as u64,
        extracted,
    })
}

/// Re-verify an already materialized plaintext payload against its manifest.
/// Restore calls this before and after the same-filesystem staging rename so
/// activation never points at a partial or modified generation.
pub fn verify_payload_directory(manifest: &BackupManifest, payload_root: &Path) -> Result<usize> {
    validate_manifest(manifest)?;
    validate_source_directory(payload_root, "backup payload root")?;
    for component in BackupComponent::ALL {
        validate_source_directory(
            &payload_root.join(component.as_str()),
            &format!("{} backup component", component.as_str()),
        )?;
    }

    let actual_files = collect_payload_files(payload_root)?;
    if actual_files != manifest.files {
        bail!("materialized backup payload does not match its manifest");
    }

    let db = Db::open_read_only(&payload_root.join(BackupComponent::Rocksdb.as_str()))
        .context("open materialized RocksDB checkpoint")?;
    let available_blobs = validate_blob_tree(manifest, payload_root)?;
    validate_referenced_blobs(&db, &available_blobs)
}

fn write_encrypted_archive(
    output: &mut File,
    payload_root: &Path,
    manifest: &BackupManifest,
    encryption: &BackupEncryption,
) -> Result<()> {
    let verification_recipient = encryption.verification_identity.to_public();
    let encryptor =
        if verification_recipient.to_string() == encryption.recovery_recipient.to_string() {
            age::Encryptor::with_recipients(std::iter::once(
                &encryption.recovery_recipient as &dyn age::Recipient,
            ))
        } else {
            age::Encryptor::with_recipients(
                [
                    &encryption.recovery_recipient as &dyn age::Recipient,
                    &verification_recipient as &dyn age::Recipient,
                ]
                .into_iter(),
            )
        }
        .context("construct age backup encryptor")?;
    let encrypted = encryptor
        .wrap_output(BufWriter::new(output))
        .context("write age backup header")?;
    let compressed =
        zstd::stream::write::Encoder::new(encrypted, 6).context("construct zstd backup encoder")?;
    let mut archive = tar::Builder::new(compressed);
    archive.mode(tar::HeaderMode::Deterministic);

    let manifest_bytes = serde_json::to_vec_pretty(manifest).context("encode backup manifest")?;
    append_bytes(
        &mut archive,
        MANIFEST_PATH,
        &manifest_bytes,
        0o600,
        manifest.created_at.timestamp(),
    )?;
    for file in &manifest.files {
        let source = payload_root.join(portable_to_path(&file.path)?);
        let archive_path = format!("{PAYLOAD_PREFIX}/{}", file.path);
        append_file(
            &mut archive,
            &archive_path,
            &source,
            file,
            manifest.created_at.timestamp(),
        )?;
    }
    archive.finish().context("finish backup tar stream")?;
    let compressed = archive.into_inner().context("finish backup tar writer")?;
    let encrypted = compressed.finish().context("finish zstd backup stream")?;
    let mut output = encrypted.finish().context("finish age backup stream")?;
    output.flush().context("flush encrypted backup")?;
    Ok(())
}

fn append_bytes<W: Write>(
    archive: &mut tar::Builder<W>,
    path: &str,
    bytes: &[u8],
    mode: u32,
    mtime: i64,
) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(bytes.len() as u64);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(mtime.max(0) as u64);
    header.set_cksum();
    archive
        .append_data(&mut header, path, bytes)
        .with_context(|| format!("append backup entry `{path}`"))
}

fn append_file<W: Write>(
    archive: &mut tar::Builder<W>,
    archive_path: &str,
    source: &Path,
    manifest: &BackupFileManifest,
    mtime: i64,
) -> Result<()> {
    let input = File::open(source)
        .with_context(|| format!("open staged backup file {}", source.display()))?;
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(manifest.size);
    header.set_mode(manifest.mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(mtime.max(0) as u64);
    header.set_cksum();
    archive
        .append_data(&mut header, archive_path, input)
        .with_context(|| format!("append backup entry `{archive_path}`"))
}

fn validate_manifest(manifest: &BackupManifest) -> Result<()> {
    if manifest.format != BACKUP_FORMAT || manifest.version != BACKUP_VERSION {
        bail!(
            "unsupported complete backup format/version: {}/{}",
            manifest.format,
            manifest.version
        );
    }
    if manifest.backend != "rocksdb"
        || manifest.archive != "tar"
        || manifest.compression != "zstd"
        || manifest.encryption != "age-x25519"
    {
        bail!("backup manifest declares an unsupported storage or envelope format");
    }
    if manifest.layout_version.is_empty() || manifest.application_version.is_empty() {
        bail!("backup manifest version metadata cannot be empty");
    }
    if manifest.files.len() > MAX_FILE_COUNT {
        bail!("backup manifest exceeds the maximum file count {MAX_FILE_COUNT}");
    }
    let component_names: Vec<_> = manifest
        .components
        .iter()
        .map(|component| component.component)
        .collect();
    if component_names != BackupComponent::ALL {
        bail!("backup manifest must contain each required component exactly once and in order");
    }

    let mut previous: Option<&str> = None;
    let mut total_bytes = 0_u64;
    let mut totals: BTreeMap<BackupComponent, (u64, u64)> = BackupComponent::ALL
        .into_iter()
        .map(|component| (component, (0, 0)))
        .collect();
    for file in &manifest.files {
        validate_portable_path(&file.path)?;
        if previous.is_some_and(|previous| previous >= file.path.as_str()) {
            bail!("backup manifest file paths must be strictly sorted and unique");
        }
        previous = Some(&file.path);
        if BackupComponent::from_path(&file.path) != Some(file.component) {
            bail!("backup file component does not match path `{}`", file.path);
        }
        if file.mode & !0o777 != 0 {
            bail!("backup file mode is invalid for `{}`", file.path);
        }
        if file.sha256.len() != 64
            || !file
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            bail!(
                "backup file checksum is not lowercase SHA-256 for `{}`",
                file.path
            );
        }
        total_bytes = total_bytes
            .checked_add(file.size)
            .context("backup payload byte count overflow")?;
        let total = totals
            .get_mut(&file.component)
            .expect("all components seeded");
        total.0 += 1;
        total.1 = total
            .1
            .checked_add(file.size)
            .context("backup component byte count overflow")?;
    }
    if total_bytes != manifest.payload_bytes {
        bail!("backup payload byte total does not match file manifest");
    }
    for component in &manifest.components {
        let actual = totals
            .get(&component.component)
            .expect("all components seeded");
        if *actual != (component.files, component.bytes) {
            bail!(
                "backup component totals do not match for {}",
                component.component.as_str()
            );
        }
    }
    Ok(())
}

fn collect_payload_files(payload_root: &Path) -> Result<Vec<BackupFileManifest>> {
    let mut paths = Vec::new();
    collect_regular_files(payload_root, &mut paths)?;
    paths.sort();
    if paths.len() > MAX_FILE_COUNT {
        bail!("backup payload exceeds the maximum file count {MAX_FILE_COUNT}");
    }
    paths
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(payload_root)
                .expect("collected below root");
            let portable = path_to_portable(relative)?;
            let component = BackupComponent::from_path(&portable)
                .with_context(|| format!("backup file has unknown component `{portable}`"))?;
            let metadata = fs::metadata(&path)?;
            Ok(BackupFileManifest {
                path: portable,
                component,
                size: metadata.len(),
                sha256: sha256_file(&path)?,
                mode: file_mode(&metadata),
            })
        })
        .collect()
}

fn collect_regular_files(current: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(current)
        .with_context(|| format!("read backup staging directory {}", current.display()))?
        .collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            bail!("backup source contains a symbolic link: {}", path.display());
        }
        if metadata.is_dir() {
            collect_regular_files(&path, output)?;
        } else if metadata.is_file() {
            output.push(path);
        } else {
            bail!("backup source contains a special file: {}", path.display());
        }
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    validate_source_directory(source, "backup component")?;
    copy_tree_inner(source, destination)
}

fn copy_tree_inner(source: &Path, destination: &Path) -> Result<()> {
    prepare_private_directory(destination)?;
    let mut entries: Vec<_> = fs::read_dir(source)
        .with_context(|| format!("read backup source {}", source.display()))?
        .collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path)?;
        if metadata.file_type().is_symlink() {
            bail!(
                "backup source contains a symbolic link: {}",
                source_path.display()
            );
        }
        if metadata.is_dir() {
            copy_tree_inner(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            let mut input = File::open(&source_path)?;
            let mut output = create_private_file(&destination_path)?;
            io::copy(&mut input, &mut output)?;
            output.sync_all()?;
            set_file_mode(&destination_path, file_mode(&metadata))?;
        } else {
            bail!(
                "backup source contains a special file: {}",
                source_path.display()
            );
        }
    }
    sync_directory(destination)?;
    Ok(())
}

fn validate_blob_tree(manifest: &BackupManifest, payload_root: &Path) -> Result<BTreeSet<String>> {
    let mut hashes = BTreeSet::new();
    for file in manifest
        .files
        .iter()
        .filter(|file| file.component == BackupComponent::Blobs)
    {
        let parts: Vec<_> = file.path.split('/').collect();
        if parts.len() != 4 || parts[0] != "blobs" || parts[1] != "uploads" {
            bail!(
                "unexpected file in content-addressed blob tree: `{}`",
                file.path
            );
        }
        let shard = parts[2];
        let tail = parts[3]
            .strip_suffix(".gz")
            .with_context(|| format!("blob file lacks .gz suffix: `{}`", file.path))?;
        let hash = format!("{shard}{tail}");
        if shard.len() != 2
            || tail.len() != 38
            || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            bail!("invalid content-addressed blob path: `{}`", file.path);
        }
        let hash = hash.to_ascii_lowercase();
        if !hashes.insert(hash.clone()) {
            bail!("duplicate content-addressed blob `{hash}`");
        }
        let path = payload_root.join(portable_to_path(&file.path)?);
        let actual = sha1_gzip_payload(&path)?;
        if actual != hash {
            bail!("blob content hash does not match path for `{}`", file.path);
        }
    }
    Ok(hashes)
}

fn validate_referenced_blobs(db: &Db, available: &BTreeSet<String>) -> Result<usize> {
    let store = RocksdbStore::new(db.clone());
    let triples = store.triple_source();
    let mut referenced = BTreeSet::new();
    for predicate in [SBOL2_HASH, LEGACY_ATTACHMENT_HASH] {
        for triple in triples
            .scan_pattern(None, Some(predicate), None, None, i64::MAX)
            .with_context(|| format!("scan attachment hashes for predicate {predicate}"))?
        {
            let ObjectTerm::Literal { value, .. } = triple.object else {
                bail!("attachment hash predicate has a non-literal object");
            };
            let hash = value.to_ascii_lowercase();
            if hash.len() != 40 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                bail!("RocksDB references an invalid attachment hash `{value}`");
            }
            if !available.contains(&hash) {
                bail!("RocksDB references missing attachment blob `{hash}`");
            }
            referenced.insert(hash);
        }
    }
    Ok(referenced.len())
}

fn sha1_gzip_payload(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    let mut decoder = GzDecoder::new(BufReader::new(file));
    let mut digest = Sha1::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let read = decoder
            .read(&mut buffer)
            .with_context(|| format!("decompress blob {}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut input = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}

fn normalized_tar_path<R: Read>(entry: &tar::Entry<'_, R>) -> Result<String> {
    let path = entry.path().context("decode backup tar path")?;
    path_to_portable(&path)
}

fn path_to_portable(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                parts.push(value.to_str().context("backup paths must be valid UTF-8")?)
            }
            _ => bail!(
                "backup path is not a safe relative path: {}",
                path.display()
            ),
        }
    }
    if parts.is_empty() {
        bail!("backup path cannot be empty");
    }
    Ok(parts.join("/"))
}

fn portable_to_path(path: &str) -> Result<PathBuf> {
    validate_portable_path(path)?;
    Ok(path.split('/').collect())
}

fn validate_portable_path(path: &str) -> Result<()> {
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

fn validate_source_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("{label} must be a real directory: {}", path.display());
    }
    Ok(())
}

fn prepare_private_directory(path: &Path) -> Result<()> {
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

fn create_private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    set_open_mode(&mut options, 0o600);
    options
        .open(path)
        .with_context(|| format!("create private backup file {}", path.display()))
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .with_context(|| format!("open directory for sync {}", path.display()))?
        .sync_all()
        .with_context(|| format!("sync directory {}", path.display()))
}

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o777
}

#[cfg(unix)]
fn verify_private_file_mode(path: &Path, metadata: &fs::Metadata) -> Result<()> {
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
fn verify_private_file_mode(_path: &Path, _metadata: &fs::Metadata) -> Result<()> {
    Ok(())
}

#[cfg(not(unix))]
fn file_mode(_metadata: &fs::Metadata) -> u32 {
    0o600
}

#[cfg(unix)]
fn set_open_mode(options: &mut OpenOptions, mode: u32) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(mode);
}

#[cfg(not(unix))]
fn set_open_mode(_options: &mut OpenOptions, _mode: u32) {}

#[cfg(unix)]
fn set_file_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_file_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_directory_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_directory_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use age::secrecy::ExposeSecret;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use sbol_db_core::SerializationFormat;
    use sbol_db_storage::{GraphWriteMode, SbolStore};

    fn write_blob(root: &Path, bytes: &[u8]) -> String {
        let hash = hex::encode(Sha1::digest(bytes));
        let directory = root.join("uploads").join(&hash[..2]);
        fs::create_dir_all(&directory).unwrap();
        let output = File::create(directory.join(format!("{}.gz", &hash[2..]))).unwrap();
        let mut gzip = GzEncoder::new(output, Compression::default());
        gzip.write_all(bytes).unwrap();
        gzip.finish().unwrap();
        hash
    }

    async fn fixture(with_blob: bool) -> (TempDir, Db, String) {
        let root = tempfile::tempdir().unwrap();
        for component in ["blobs", "search", "acme", "backups"] {
            fs::create_dir_all(root.path().join(component)).unwrap();
        }
        fs::write(root.path().join("search/meta.json"), b"search-state").unwrap();
        fs::write(
            root.path().join("acme/account-key"),
            b"private-account-state",
        )
        .unwrap();
        let db = Db::open(&root.path().join("live-rocksdb")).unwrap();
        let hash = if with_blob {
            write_blob(&root.path().join("blobs"), b"complete attachment")
        } else {
            hex::encode(Sha1::digest(b"missing attachment"))
        };
        let store = RocksdbStore::new(db.clone());
        store
            .graph_store_write(
                "https://example.org/graph",
                &format!("<https://example.org/attachment> <{SBOL2_HASH}> \"{hash}\" ."),
                SerializationFormat::NTriples,
                GraphWriteMode::Replace,
            )
            .await
            .unwrap();
        (root, db, hash)
    }

    fn encryption() -> (BackupEncryption, x25519::Identity) {
        let recovery = x25519::Identity::generate();
        let verification = x25519::Identity::generate();
        (
            BackupEncryption::new(recovery.to_public(), verification),
            recovery,
        )
    }

    #[tokio::test]
    async fn creates_decrypts_and_semantically_verifies_complete_backup() {
        let (root, db, hash) = fixture(true).await;
        let (encryption, recovery) = encryption();
        let generation = Uuid::new_v4();
        let created = create_complete_backup(
            CompleteBackupSource {
                db: &db,
                blobs_root: &root.path().join("blobs"),
                search_root: &root.path().join("search"),
                acme_root: &root.path().join("acme"),
                generation,
                layout_version: "1",
                application_version: "test",
            },
            &root.path().join("backups"),
            &encryption,
        )
        .unwrap();

        assert!(created.path.is_file());
        assert!(!created.reused);
        assert_eq!(created.referenced_blobs, 1);
        assert_eq!(created.artifact_sha256.len(), 64);
        let verified =
            verify_encrypted_backup(&created.path, &recovery, &root.path().join("backups"))
                .unwrap();
        assert_eq!(verified.manifest().source_generation, generation);
        assert_eq!(verified.referenced_blobs(), 1);
        assert!(verified
            .payload_root()
            .join(format!("blobs/uploads/{}/{}.gz", &hash[..2], &hash[2..]))
            .is_file());
        assert!(verified.payload_root().join("search/meta.json").is_file());
        assert!(verified.payload_root().join("acme/account-key").is_file());

        let retried = create_complete_backup_with_id(
            CompleteBackupSource {
                db: &db,
                blobs_root: &root.path().join("blobs"),
                search_root: &root.path().join("search"),
                acme_root: &root.path().join("acme"),
                generation,
                layout_version: "1",
                application_version: "test",
            },
            &root.path().join("backups"),
            &encryption,
            created.backup_id,
            created.created_at,
        )
        .unwrap();
        assert!(retried.reused);
        assert_eq!(retried.path, created.path);
        assert_eq!(retried.artifact_sha256, created.artifact_sha256);
    }

    #[tokio::test]
    async fn uploads_and_semantically_verifies_object_store_readback() {
        use object_store::memory::InMemory;

        let (root, db, _hash) = fixture(true).await;
        let recovery = x25519::Identity::generate();
        let verification = x25519::Identity::generate();
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let repository = Arc::new(ObjectStoreBackupRepository::new(
            store.clone(),
            "memory",
            "test-bucket",
            ObjectPath::parse("registry/production").unwrap(),
        ));
        let service = CompleteBackupService::new(
            CompleteBackupConfig {
                db,
                database_root: root.path().join("live-rocksdb"),
                blobs_root: root.path().join("blobs"),
                search_root: root.path().join("search"),
                acme_root: root.path().join("acme"),
                backups_root: root.path().join("backups"),
                generation: Uuid::new_v4(),
                layout_version: "1".to_owned(),
                application_version: "test".to_owned(),
                minimum_free_bytes: 0,
                local_retention: 2,
            },
            BackupEncryption::new(recovery.to_public(), verification),
        )
        .with_repository(repository);
        let backup_id = Uuid::new_v4();
        let requested_at = Utc::now();

        let first = service.create(backup_id, requested_at).await.unwrap();
        let first_remote = first.remote.as_ref().expect("remote result");
        assert!(!first_remote.reused);
        assert_eq!(first_remote.artifact_sha256, first.local.artifact_sha256);
        assert_eq!(
            store
                .head(&ObjectPath::parse(&first_remote.object_key).unwrap())
                .await
                .unwrap()
                .size,
            first.local.artifact_bytes
        );

        let retried = service.create(backup_id, requested_at).await.unwrap();
        assert!(retried.local.reused);
        assert!(retried.remote.unwrap().reused);

        let _second = service.create(Uuid::new_v4(), Utc::now()).await.unwrap();
        let third = service.create(Uuid::new_v4(), Utc::now()).await.unwrap();
        let retention = third.local_retention.expect("retention report");
        assert_eq!(retention.retained_artifacts, 2);
        assert_eq!(retention.pruned_artifacts, 1);
        let local_artifacts = fs::read_dir(root.path().join("backups"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "age"))
            .count();
        assert_eq!(local_artifacts, 2);
    }

    #[tokio::test]
    async fn refuses_backup_when_the_operational_disk_reserve_would_be_consumed() {
        let (root, db, _hash) = fixture(true).await;
        let available = fs2::available_space(root.path()).unwrap();
        let config = CompleteBackupConfig {
            db,
            database_root: root.path().join("live-rocksdb"),
            blobs_root: root.path().join("blobs"),
            search_root: root.path().join("search"),
            acme_root: root.path().join("acme"),
            backups_root: root.path().join("backups"),
            generation: Uuid::new_v4(),
            layout_version: "1".to_owned(),
            application_version: "test".to_owned(),
            minimum_free_bytes: available.saturating_add(1),
            local_retention: 2,
        };

        let error = backup_disk_preflight(&config).unwrap_err().to_string();
        assert!(error.contains("insufficient disk space"), "got: {error}");
    }

    #[tokio::test]
    async fn refuses_to_publish_when_a_referenced_blob_is_missing() {
        let (root, db, _hash) = fixture(false).await;
        let (encryption, _recovery) = encryption();
        let result = create_complete_backup(
            CompleteBackupSource {
                db: &db,
                blobs_root: &root.path().join("blobs"),
                search_root: &root.path().join("search"),
                acme_root: &root.path().join("acme"),
                generation: Uuid::new_v4(),
                layout_version: "1",
                application_version: "test",
            },
            &root.path().join("backups"),
            &encryption,
        );
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("read-back verification"));
        let published = fs::read_dir(root.path().join("backups"))
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.path().extension().is_some_and(|ext| ext == "age"));
        assert!(!published);
    }

    #[test]
    fn parses_age_keygen_identity_files_without_exposing_secret_in_errors() {
        let identity = x25519::Identity::generate();
        let encoded = identity.to_string();
        let file = format!(
            "# created: now\n# public key: {}\n{}\n",
            identity.to_public(),
            encoded.expose_secret()
        );
        let parsed = parse_x25519_identity(&file).unwrap();
        assert_eq!(parsed.to_public(), identity.to_public());
    }

    #[test]
    fn creates_and_reloads_a_private_local_verification_identity() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("verification.agekey");
        let recovery = x25519::Identity::generate();
        let first = load_or_create_encryption(&recovery.to_public().to_string(), &path).unwrap();
        let second = load_or_create_encryption(&recovery.to_public().to_string(), &path).unwrap();

        assert!(path.is_file());
        assert_eq!(
            first.verification_identity().to_public(),
            second.verification_identity().to_public()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn repository_urls_require_a_supported_scheme_bucket_and_prefix() {
        assert!(ObjectStoreBackupRepository::from_url("https://bucket/prefix").is_err());
        assert!(ObjectStoreBackupRepository::from_url("s3://bucket").is_err());
        assert!(ObjectStoreBackupRepository::from_url("gs:///prefix").is_err());
        assert!(ObjectStoreBackupRepository::from_url("s3://user:secret@bucket/prefix").is_err());
    }
}
