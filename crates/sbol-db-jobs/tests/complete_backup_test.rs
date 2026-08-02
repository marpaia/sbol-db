//! End-to-end complete-backup handler test without external services.

use std::fs;
use std::sync::Arc;

use age::x25519;
use sbol_db_backup::{BackupEncryption, CompleteBackupConfig, CompleteBackupService};
use sbol_db_jobs::handlers::CompleteBackupHandler;
use sbol_db_jobs::{BackupTrigger, CompleteBackupPayload, JobContext, JobHandler};
use sbol_db_rocksdb::Db;
use sbol_db_sqlite::{connect_and_migrate, SqliteJobRepository, SqliteStore};
use sbol_db_storage::{JobQueue, NewJob, SbolStore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[tokio::test]
async fn handler_publishes_one_verified_artifact_and_reuses_it_on_retry() {
    let root = tempfile::tempdir().unwrap();
    for component in ["blobs", "search", "acme", "backups"] {
        fs::create_dir_all(root.path().join(component)).unwrap();
    }
    let rocksdb = Db::open(&root.path().join("rocksdb")).unwrap();
    rocksdb
        .put_cf("app_config", b"instance", b"durable-state")
        .unwrap();
    let recovery = x25519::Identity::generate();
    let verification = x25519::Identity::generate();
    let backups = Arc::new(CompleteBackupService::new(
        CompleteBackupConfig {
            db: rocksdb,
            database_root: root.path().join("rocksdb"),
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
    ));

    let sqlite_url = format!("sqlite://{}", root.path().join("jobs.db").display());
    let pool = connect_and_migrate(&sqlite_url).await.unwrap();
    let service: Arc<dyn SbolStore> = Arc::new(SqliteStore::new(pool.clone()));
    let jobs: Arc<dyn JobQueue> = Arc::new(SqliteJobRepository::new(pool));
    let payload = CompleteBackupPayload::new(BackupTrigger::Manual, Some("test-admin".to_owned()));
    let job = jobs
        .enqueue(NewJob::new(
            "complete_backup",
            serde_json::to_value(&payload).unwrap(),
        ))
        .await
        .unwrap()
        .into_job();
    let context = || JobContext {
        job_id: job.id,
        worker_id: Arc::from("test-worker"),
        attempt: 1,
        service: service.clone(),
        jobs: jobs.clone(),
        cancel: CancellationToken::new(),
        search: None,
        vector_indexes: None,
        config: None,
        backups: Some(backups.clone()),
    };

    let first = CompleteBackupHandler
        .run(context(), payload.clone())
        .await
        .unwrap()
        .result
        .unwrap();
    let second = CompleteBackupHandler
        .run(context(), payload)
        .await
        .unwrap()
        .result
        .unwrap();

    assert_eq!(first["backup_id"], job.id.to_string());
    assert_eq!(first["reused"], false);
    assert_eq!(second["reused"], true);
    assert_eq!(first["artifact_sha256"], second["artifact_sha256"]);
    let artifacts = fs::read_dir(root.path().join("backups"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "age"))
        .count();
    assert_eq!(artifacts, 1);
}
