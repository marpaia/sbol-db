use super::*;

use std::fs;
use std::path::Path;

use sbol_db_backup::{
    create_complete_backup, verify_encrypted_backup, BackupEncryption, CompleteBackupSource,
};
use sbol_db_rocksdb::Db;
use uuid::Uuid;

use crate::cli::{BackendKind, RuntimeProfile};
use crate::runtime::layout::{CURRENT_FILE, LAYOUT_VERSION, VERSION_FILE};

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
    fs::write(source.path().join("search/index"), b"restored-search").expect("write search state");
    fs::write(source.path().join("acme/account"), b"restored-acme").expect("write ACME state");
    let restored_db = Db::open(&source.path().join("rocksdb")).expect("open restored database");
    restored_db
        .put_cf("meta", b"restore-test", b"restored")
        .expect("write restored marker");
    let recovery = age::x25519::Identity::generate();
    let encryption = BackupEncryption::new(recovery.to_public(), age::x25519::Identity::generate());
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
    let original_db = Db::open_read_only(original.database_path()).expect("read original database");
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
    let recovery_status = original.recovery_status().expect("read recovery status");
    assert_eq!(recovery_status.active_generation, original_generation);
    assert_eq!(recovery_status.previous_generation, Some(created.backup_id));
    assert_eq!(
        recovery_status
            .last_operation
            .as_ref()
            .map(|event| event.status),
        Some(RestoreJournalStatus::RolledBack)
    );
    assert!(
        recovery_status.history.len() >= 4,
        "staging, activation, rollback-pending, and rollback must be retained"
    );
    drop(original_db);
    drop(original);

    let fresh_data = tempfile::tempdir().expect("fresh restore tempdir");
    let fresh_layout =
        ManagedDataLayout::open(fresh_data.path()).expect("initialize fresh restore layout");
    let verified = verify_encrypted_backup(&created.path, &recovery, fresh_layout.restore_root())
        .expect("verify backup for fresh restore");
    let fresh_restore = fresh_layout
        .restore_verified(verified, &format!("RESTORE {}", created.backup_id))
        .expect("restore into pristine layout");
    assert_eq!(fresh_restore.previous_generation, None);
    assert_eq!(fresh_restore.rollback_confirmation, None);
    drop(fresh_layout);

    let restored_fresh =
        ManagedDataLayout::open(fresh_data.path()).expect("open fresh restored generation");
    let restored_fresh_db =
        Db::open_read_only(restored_fresh.database_path()).expect("read fresh restored database");
    assert_eq!(
        restored_fresh_db
            .get_cf("meta", b"restore-test")
            .expect("read fresh restored marker"),
        Some(b"restored".to_vec())
    );
    let fresh_status = restored_fresh
        .recovery_status()
        .expect("read fresh recovery status");
    assert_eq!(fresh_status.active_generation, created.backup_id);
    assert_eq!(fresh_status.previous_generation, None);
    assert_eq!(fresh_status.history.len(), 2);
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
