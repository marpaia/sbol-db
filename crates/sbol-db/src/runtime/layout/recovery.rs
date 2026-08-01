use std::fs;
use std::io::ErrorKind;

use anyhow::{bail, Context, Result};
use chrono::Utc;
use sbol_db_backup::{verify_payload_directory, VerifiedBackup};
use uuid::Uuid;

use super::{
    ManagedDataLayout, RecoveryEvent, RecoveryStatus, RestoreJournal, RestoreJournalStatus,
    RestoreOutcome, RollbackOutcome, CURRENT_FILE, LAYOUT_VERSION, MAX_RECOVERY_HISTORY,
    PREVIOUS_FILE, RESTORE_HISTORY_DIR, RESTORE_JOURNAL_FILE,
};
use crate::runtime::filesystem::{
    atomic_write, is_pristine_generation, optional_generation_pointer, prepare_directory,
    read_generation_pointer, read_optional_generation_pointer, reject_symlink, sync_directory,
    sync_tree, verify_generation_structure,
};

impl ManagedDataLayout {
    pub fn recovery_status(&self) -> Result<RecoveryStatus> {
        let active_generation =
            read_generation_pointer(&self.root.join(CURRENT_FILE), "current-generation pointer")?;
        let previous_generation = optional_generation_pointer(
            &self.root.join(PREVIOUS_FILE),
            "previous-generation pointer",
        )?;
        let last_operation = self.read_restore_journal()?;
        let mut history = self.read_recovery_history()?;
        if history.is_empty() {
            history.extend(last_operation.iter().cloned());
        }
        Ok(RecoveryStatus {
            active_generation,
            previous_generation,
            last_operation,
            history,
        })
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
        )?;
        self.record_recovery_event(journal)
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

    fn record_recovery_event(&self, event: &RecoveryEvent) -> Result<()> {
        let history_root = self.restore_root.join(RESTORE_HISTORY_DIR);
        prepare_directory(&history_root, "restore history directory")?;
        let name = format!(
            "{}-{}-{}.json",
            event.updated_at.timestamp_micros(),
            event.status.as_str(),
            Uuid::new_v4().simple()
        );
        let bytes = serde_json::to_vec_pretty(event).context("encode recovery history event")?;
        atomic_write(&history_root, &history_root.join(name), &bytes)
    }

    fn read_recovery_history(&self) -> Result<Vec<RecoveryEvent>> {
        let history_root = self.restore_root.join(RESTORE_HISTORY_DIR);
        reject_symlink(&history_root, "restore history directory")?;
        let entries = match fs::read_dir(&history_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read restore history {}", history_root.display()));
            }
        };
        let mut history = Vec::new();
        for entry in entries {
            let entry = entry.context("read restore history entry")?;
            let path = entry.path();
            reject_symlink(&path, "restore history event")?;
            let metadata = entry.metadata().context("read restore history metadata")?;
            if !metadata.is_file() || metadata.len() > 64 * 1024 {
                continue;
            }
            let event = serde_json::from_slice::<RecoveryEvent>(
                &fs::read(&path)
                    .with_context(|| format!("read restore history event {}", path.display()))?,
            )
            .with_context(|| format!("decode restore history event {}", path.display()))?;
            history.push(event);
        }
        history.sort_by_key(|event| std::cmp::Reverse(event.updated_at));
        history.truncate(MAX_RECOVERY_HISTORY);
        Ok(history)
    }
}
