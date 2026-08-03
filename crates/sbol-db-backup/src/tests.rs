use super::*;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use age::secrecy::ExposeSecret;
use age::x25519;
use chrono::Utc;
use flate2::write::GzEncoder;
use flate2::Compression;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt};
use sbol_db_core::SerializationFormat;
use sbol_db_rocksdb::{Db, RocksdbStore};
use sbol_db_storage::{GraphWriteMode, SbolStore};
use sha1::{Digest, Sha1};
use tempfile::TempDir;
use uuid::Uuid;

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
    fs::write(
        root.path().join("blobs/uploads/logo_uploaded.svg"),
        b"<svg>classic instance logo</svg>",
    )
    .unwrap();
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
    assert!(created.missing_referenced_blobs.is_empty());
    assert_eq!(created.artifact_sha256.len(), 64);
    let verified =
        verify_encrypted_backup(&created.path, &recovery, &root.path().join("backups")).unwrap();
    assert_eq!(verified.manifest().source_generation, generation);
    assert_eq!(verified.referenced_blobs(), 1);
    assert!(verified
        .payload_root()
        .join(format!("blobs/uploads/{}/{}.gz", &hash[..2], &hash[2..]))
        .is_file());
    assert_eq!(
        fs::read(
            verified
                .payload_root()
                .join("blobs/uploads/logo_uploaded.svg")
        )
        .unwrap(),
        b"<svg>classic instance logo</svg>"
    );
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
async fn authenticates_a_source_missing_reference_without_inventing_blob_bytes() {
    let (root, db, hash) = fixture(false).await;
    let (encryption, recovery) = encryption();
    let created = create_complete_backup(
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
    )
    .unwrap();
    assert_eq!(created.referenced_blobs, 1);
    assert_eq!(created.missing_referenced_blobs, vec![hash.clone()]);
    assert!(!root
        .path()
        .join(format!("blobs/uploads/{}/{}.gz", &hash[..2], &hash[2..]))
        .exists());

    let verified =
        verify_encrypted_backup(&created.path, &recovery, &root.path().join("backups")).unwrap();
    assert_eq!(verified.manifest().missing_referenced_blobs, vec![hash]);
}

#[tokio::test]
async fn rejects_unknown_non_content_addressed_upload_files() {
    let (root, db, _hash) = fixture(true).await;
    fs::write(
        root.path().join("blobs/uploads/unexpected.txt"),
        b"not an application-managed legacy logo",
    )
    .unwrap();
    let (encryption, _recovery) = encryption();
    let error = create_complete_backup(
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
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("unexpected file in content-addressed blob tree"),
        "got: {error:#}"
    );
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
