use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use sbol_db_rocksdb::Db;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uuid::Uuid;

use super::payload::{
    audit_referenced_blobs, collect_payload_files, normalized_tar_path, portable_to_path,
    sha256_file, validate_blob_tree,
};
use crate::filesystem::{
    create_private_file, prepare_private_directory, set_file_mode, validate_portable_path,
    validate_source_directory,
};
use crate::manifest::{
    BackupComponent, BackupComponentManifest, BackupManifest, BACKUP_FORMAT, BACKUP_VERSION,
    MANIFEST_PATH, MAX_FILE_COUNT, MAX_MANIFEST_BYTES, PAYLOAD_PREFIX,
};

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
    pub missing_referenced_blobs: Vec<String>,
    pub components: Vec<BackupComponentManifest>,
}

/// A decrypted backup that remains extracted until dropped. Restore can stage
/// this payload directly and atomically activate it in a later phase.
pub struct VerifiedBackup {
    pub(crate) manifest: BackupManifest,
    pub(crate) artifact_sha256: String,
    pub(crate) artifact_bytes: u64,
    pub(crate) referenced_blobs: u64,
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
            missing_referenced_blobs: self.manifest.missing_referenced_blobs.clone(),
            components: self.manifest.components.clone(),
        }
    }

    /// Keep the verified extraction directory for staged restore.
    pub fn into_extracted_path(self) -> PathBuf {
        self.extracted.keep()
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
    let audit = audit_referenced_blobs(&db, &available_blobs)?;
    if audit.missing != manifest.missing_referenced_blobs {
        bail!(
            "materialized backup missing-reference set does not match its authenticated manifest: manifest={}, actual={}",
            manifest.missing_referenced_blobs.len(),
            audit.missing.len()
        );
    }
    Ok(audit.referenced)
}

pub(super) fn validate_manifest(manifest: &BackupManifest) -> Result<()> {
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
    if manifest.missing_referenced_blobs.len() > MAX_FILE_COUNT {
        bail!("backup manifest exceeds the maximum missing-reference count {MAX_FILE_COUNT}");
    }
    let mut previous_missing: Option<&str> = None;
    for hash in &manifest.missing_referenced_blobs {
        if hash.len() != 40
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            bail!("backup manifest contains an invalid missing attachment hash");
        }
        if previous_missing.is_some_and(|previous| previous >= hash.as_str()) {
            bail!("backup manifest missing attachment hashes must be strictly sorted and unique");
        }
        previous_missing = Some(hash);
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
