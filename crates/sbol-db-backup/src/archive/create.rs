use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use uuid::Uuid;

use super::payload::{collect_payload_files, copy_tree, portable_to_path};
use super::verify::{validate_manifest, verify_encrypted_backup, VerifiedBackup};
use crate::encryption::BackupEncryption;
use crate::filesystem::{prepare_private_directory, sync_directory, validate_source_directory};
use crate::manifest::{
    BackupComponent, BackupComponentManifest, BackupFileManifest, BackupManifest,
    CompleteBackupSource, BACKUP_FORMAT, BACKUP_VERSION, MANIFEST_PATH, PAYLOAD_PREFIX,
};
use crate::types::CreatedBackup;

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

fn write_encrypted_archive(
    output: &mut File,
    payload_root: &Path,
    manifest: &BackupManifest,
    encryption: &BackupEncryption,
) -> Result<()> {
    let verification_recipient = encryption.verification_identity().to_public();
    let encryptor =
        if verification_recipient.to_string() == encryption.recovery_recipient().to_string() {
            age::Encryptor::with_recipients(std::iter::once(
                encryption.recovery_recipient() as &dyn age::Recipient
            ))
        } else {
            age::Encryptor::with_recipients(
                [
                    encryption.recovery_recipient() as &dyn age::Recipient,
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
