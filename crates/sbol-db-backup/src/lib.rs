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

use age::x25519;
use anyhow::{bail, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use flate2::read::GzDecoder;
use sbol_db_core::ObjectTerm;
use sbol_db_rocksdb::{Db, RocksdbStore};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
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
    pub path: PathBuf,
    pub artifact_sha256: String,
    pub artifact_bytes: u64,
    pub payload_bytes: u64,
    pub files: u64,
    pub referenced_blobs: u64,
    pub verified_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct VerifiedBackupReport {
    pub status: &'static str,
    pub backup_id: Uuid,
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
    prepare_private_directory(backup_root)?;
    validate_source_directory(source.blobs_root, "blob root")?;
    validate_source_directory(source.search_root, "search root")?;
    validate_source_directory(source.acme_root, "ACME root")?;

    let backup_id = Uuid::new_v4();
    let created_at = Utc::now();
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
    let referenced_blobs = verified.referenced_blobs;
    let artifact_sha256 = verified.artifact_sha256.clone();
    let artifact_bytes = verified.artifact_bytes;
    drop(verified);

    let stamp = created_at
        .to_rfc3339_opts(SecondsFormat::Secs, true)
        .replace([':', '-'], "");
    let file_name = format!("sbol-db-{stamp}-{backup_id}.sbolbackup.age");
    let final_path = backup_root.join(file_name);
    partial
        .persist_noclobber(&final_path)
        .map_err(|error| error.error)
        .with_context(|| format!("publish verified backup at {}", final_path.display()))?;
    sync_directory(backup_root)?;

    Ok(CreatedBackup {
        backup_id,
        path: final_path,
        artifact_sha256,
        artifact_bytes,
        payload_bytes,
        files: manifest.files.len() as u64,
        referenced_blobs,
        verified_at: Utc::now(),
    })
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
    let artifact_bytes = fs::metadata(artifact)
        .with_context(|| format!("inspect backup artifact {}", artifact.display()))?
        .len();
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

    let db = Db::open_read_only(&payload_root.join(BackupComponent::Rocksdb.as_str()))
        .context("open extracted RocksDB checkpoint")?;
    let available_blobs = validate_blob_tree(&manifest, &payload_root)?;
    let referenced_blobs = validate_referenced_blobs(&db, &available_blobs)?;

    Ok(VerifiedBackup {
        manifest,
        artifact_sha256,
        artifact_bytes,
        referenced_blobs: referenced_blobs as u64,
        extracted,
    })
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
}
