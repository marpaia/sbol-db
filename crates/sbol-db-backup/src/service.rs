use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use sbol_db_rocksdb::Db;
use uuid::Uuid;

use crate::archive::{create_complete_backup_with_id, sha256_file};
use crate::encryption::BackupEncryption;
use crate::filesystem::{set_file_mode, sync_directory, validate_source_directory};
use crate::manifest::{CompleteBackupSource, MAX_MANIFEST_BYTES};
use crate::repository::BackupRepository;
use crate::types::{BackupDiskPreflight, CompletedBackup, LocalRetentionReport, PublishedBackup};

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

pub(crate) fn backup_disk_preflight(config: &CompleteBackupConfig) -> Result<BackupDiskPreflight> {
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
