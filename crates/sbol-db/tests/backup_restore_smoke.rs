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

#[test]
fn create_and_restore_an_offline_seed_through_the_cli() {
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("source");
    let backups = fixture.path().join("backups");
    let restored = fixture.path().join("restored");
    for component in ["blobs", "search", "acme"] {
        fs::create_dir_all(source.join(component)).unwrap();
    }
    fs::create_dir_all(&backups).unwrap();
    fs::write(source.join("search/index"), b"search-state").unwrap();
    let db = Db::open(&source.join("rocksdb")).unwrap();
    db.put_cf("meta", b"seed", b"exact-source").unwrap();
    drop(db);

    let identity_file = fixture.path().join("recovery.agekey");
    let keygen = Command::cargo_bin("sbol-db")
        .unwrap()
        .arg("backup")
        .arg("keygen")
        .arg("--identity-file")
        .arg(&identity_file)
        .output()
        .unwrap();
    assert!(keygen.status.success());
    let keygen_json: serde_json::Value = serde_json::from_slice(&keygen.stdout).unwrap();
    assert!(keygen_json["recipient"]
        .as_str()
        .unwrap()
        .starts_with("age1"));

    let create = Command::cargo_bin("sbol-db")
        .unwrap()
        .arg("backup")
        .arg("create")
        .arg("--database-root")
        .arg(source.join("rocksdb"))
        .arg("--blobs-root")
        .arg(source.join("blobs"))
        .arg("--search-root")
        .arg(source.join("search"))
        .arg("--acme-root")
        .arg(source.join("acme"))
        .arg("--backup-root")
        .arg(&backups)
        .arg("--identity-file")
        .arg(&identity_file)
        .output()
        .unwrap();
    assert!(
        create.status.success(),
        "{}",
        String::from_utf8_lossy(&create.stderr)
    );
    let created: serde_json::Value = serde_json::from_slice(&create.stdout).unwrap();
    let backup_id = created["backup_id"].as_str().unwrap();
    let artifact = std::path::PathBuf::from(created["path"].as_str().unwrap());
    assert!(artifact.is_file());

    let restore = Command::cargo_bin("sbol-db")
        .unwrap()
        .arg("backup")
        .arg("restore")
        .arg("--artifact")
        .arg(&artifact)
        .arg("--identity-file")
        .arg(&identity_file)
        .arg("--data-dir")
        .arg(&restored)
        .arg("--confirmation")
        .arg(format!("RESTORE {backup_id}"))
        .arg("--remove-artifact-on-success")
        .arg("--remove-identity-on-success")
        .output()
        .unwrap();
    assert!(
        restore.status.success(),
        "{}",
        String::from_utf8_lossy(&restore.stderr)
    );
    assert!(!artifact.exists());
    assert!(!identity_file.exists());

    let restored_db =
        Db::open_read_only(&restored.join("generations").join(backup_id).join("rocksdb")).unwrap();
    assert_eq!(
        restored_db.get_cf("meta", b"seed").unwrap(),
        Some(b"exact-source".to_vec())
    );
    assert_eq!(
        fs::read(
            restored
                .join("generations")
                .join(backup_id)
                .join("search/index")
        )
        .unwrap(),
        b"search-state"
    );
}

#[cfg(unix)]
fn set_private_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &std::path::Path) {}
