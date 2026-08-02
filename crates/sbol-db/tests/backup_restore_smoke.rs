use std::fs;

use age::secrecy::ExposeSecret;
use assert_cmd::Command;
use sbol_db_backup::{create_complete_backup, BackupEncryption, CompleteBackupSource};
use sbol_db_rocksdb::Db;
use uuid::Uuid;

#[test]
fn verify_and_restore_a_complete_artifact_through_the_cli() {
    let fixture = tempfile::tempdir().unwrap();
    for component in ["blobs", "search", "acme", "backups", "verify"] {
        fs::create_dir_all(fixture.path().join(component)).unwrap();
    }
    fs::write(fixture.path().join("search/index"), b"search-state").unwrap();
    fs::write(fixture.path().join("acme/account"), b"acme-state").unwrap();
    let db = Db::open(&fixture.path().join("rocksdb")).unwrap();
    db.put_cf("meta", b"acceptance", b"restored").unwrap();
    let recovery = age::x25519::Identity::generate();
    let encryption = BackupEncryption::new(recovery.to_public(), age::x25519::Identity::generate());
    let created = create_complete_backup(
        CompleteBackupSource {
            db: &db,
            blobs_root: &fixture.path().join("blobs"),
            search_root: &fixture.path().join("search"),
            acme_root: &fixture.path().join("acme"),
            generation: Uuid::new_v4(),
            layout_version: "2",
            application_version: "acceptance-test",
        },
        &fixture.path().join("backups"),
        &encryption,
    )
    .unwrap();
    drop(db);

    let identity_file = fixture.path().join("recovery.agekey");
    fs::write(
        &identity_file,
        format!("{}\n", recovery.to_string().expose_secret()),
    )
    .unwrap();
    set_private_permissions(&identity_file);

    let mut verify = Command::cargo_bin("sbol-db").unwrap();
    let verify_output = verify
        .arg("backup")
        .arg("verify")
        .arg("--artifact")
        .arg(&created.path)
        .arg("--identity-file")
        .arg(&identity_file)
        .arg("--staging-dir")
        .arg(fixture.path().join("verify"))
        .output()
        .unwrap();
    assert!(verify_output.status.success());
    assert!(String::from_utf8(verify_output.stdout)
        .unwrap()
        .contains(&format!("RESTORE {}", created.backup_id)));

    let data_dir = fixture.path().join("restored-appliance");
    let mut restore = Command::cargo_bin("sbol-db").unwrap();
    let restore_output = restore
        .arg("backup")
        .arg("restore")
        .arg("--artifact")
        .arg(&created.path)
        .arg("--identity-file")
        .arg(&identity_file)
        .arg("--data-dir")
        .arg(&data_dir)
        .arg("--confirmation")
        .arg(format!("RESTORE {}", created.backup_id))
        .output()
        .unwrap();
    assert!(restore_output.status.success());
    let restore_stdout = String::from_utf8(restore_output.stdout).unwrap();
    assert!(restore_stdout.contains("\"status\": \"activated\""));
    assert!(restore_stdout.contains("\"previous_generation\": null"));

    assert_eq!(
        fs::read_to_string(data_dir.join("CURRENT")).unwrap().trim(),
        created.backup_id.to_string()
    );
    let restored_db = Db::open_read_only(
        &data_dir
            .join("generations")
            .join(created.backup_id.to_string())
            .join("rocksdb"),
    )
    .unwrap();
    assert_eq!(
        restored_db.get_cf("meta", b"acceptance").unwrap(),
        Some(b"restored".to_vec())
    );
    assert_eq!(
        fs::read(
            data_dir
                .join("generations")
                .join(created.backup_id.to_string())
                .join("acme/account")
        )
        .unwrap(),
        b"acme-state"
    );
}

#[cfg(unix)]
fn set_private_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &std::path::Path) {}
