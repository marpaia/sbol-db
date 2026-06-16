//! End-to-end test for the embedded worker against a live Postgres.
//!
//! Mirrors the pattern other integration tests in this workspace use: one
//! process-wide mutex around the database, TRUNCATE between cases.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use sbol_db_core::SerializationFormat;
use sbol_db_jobs::{
    default_registry, handlers::ImportDocumentPayload, HandlerError, JobContext, JobHandler,
    JobOutcome, JobRegistry, Worker, WorkerConfig,
};
use sbol_db_postgres::{
    connect, run_migrations, EnqueueOutcome, JobRepository, JobStatus, NewJob, SbolObjectService,
    DEFAULT_QUEUE,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{Mutex, MutexGuard};
use tokio_util::sync::CancellationToken;

const FIXTURE: &str = include_str!("../../sbol-db-postgres/tests/fixtures/simple_component.ttl");

static DB_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

async fn db_lock() -> MutexGuard<'static, ()> {
    DB_MUTEX.get_or_init(|| Mutex::new(())).lock().await
}

async fn fresh_pool() -> sbol_db_postgres::PgPool {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://sbol:sbol@localhost:5432/sbol".to_owned());
    let pool = connect(&database_url).await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    sqlx::query(
        "TRUNCATE sbol_graphs, sbol_objects, sbol_triples, sbol_validation_findings, \
         sbol_validation_runs, sbol_object_revisions, sbol_rdf_projection_events, \
         sbol_jobs, sbol_job_attempts RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate");
    pool
}

async fn drive_worker_until<F, Fut>(
    pool: sbol_db_postgres::PgPool,
    registry: JobRegistry,
    predicate: F,
) -> bool
where
    F: Fn(JobRepository) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let cancel = CancellationToken::new();
    let service = Arc::new(SbolObjectService::new(pool.clone()));
    let registry = Arc::new(registry);
    let config = WorkerConfig {
        concurrency: 2,
        queues: vec![DEFAULT_QUEUE.to_owned()],
        lease: Duration::from_secs(10),
        renew_interval: Duration::from_secs(3),
        reaper_interval: Duration::from_secs(2),
        poll_interval: Duration::from_millis(200),
        shutdown_grace: Duration::from_secs(5),
        ..WorkerConfig::default()
    };
    let worker = Worker::new(pool.clone(), service, registry, config);
    let cancel_clone = cancel.clone();
    let handle = tokio::spawn(async move {
        worker.run(cancel_clone).await.expect("worker run");
    });

    let repo = JobRepository::new(pool);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut hit = false;
    while tokio::time::Instant::now() < deadline {
        if predicate(repo.clone()).await {
            hit = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    cancel.cancel();
    let _ = handle.await;
    hit
}

#[tokio::test]
async fn embedded_worker_drains_import_document_job() {
    let _guard = db_lock().await;
    let pool = fresh_pool().await;
    let repo = JobRepository::new(pool.clone());

    let payload = serde_json::to_value(ImportDocumentPayload {
        body: FIXTURE.to_owned(),
        format: SerializationFormat::Turtle,
        namespace: None,
        source_uri: Some("test://embedded_worker.ttl".to_owned()),
        document_iri: None,
        name: None,
        description: None,
        created_by: Some("worker-test".to_owned()),
    })
    .unwrap();

    let outcome = repo
        .enqueue(NewJob {
            kind: "import_document".to_owned(),
            payload,
            queue: None,
            priority: None,
            max_attempts: None,
            idempotency_key: None,
            available_at: None,
            parent_job_id: None,
            correlation_id: None,
        })
        .await
        .expect("enqueue");
    let job_id = match outcome {
        EnqueueOutcome::Inserted(j) => j.id,
        EnqueueOutcome::AlreadyExists(j) => j.id,
    };

    let succeeded = drive_worker_until(pool.clone(), default_registry(), move |repo| {
        let id = job_id;
        async move {
            repo.current_status(id)
                .await
                .ok()
                .flatten()
                .map(|s| s == JobStatus::Succeeded)
                .unwrap_or(false)
        }
    })
    .await;
    assert!(succeeded, "expected the job to reach succeeded");

    let final_job = repo.get(job_id).await.expect("get").expect("present");
    assert_eq!(final_job.status, JobStatus::Succeeded);
    assert!(
        final_job.result.is_some(),
        "import_document handler should write its ImportReport into result"
    );
    let report = final_job.result.unwrap();
    assert!(report.get("graph_id").is_some());
    assert!(report.get("object_count").is_some());
}

// ---------- failure / retry path ----------

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FlakyPayload {
    msg: String,
}

struct FlakyHandler {
    attempts: Arc<AtomicUsize>,
}

#[async_trait]
impl JobHandler for FlakyHandler {
    type Payload = FlakyPayload;

    fn kind(&self) -> &'static str {
        "test_flaky"
    }

    async fn run(
        &self,
        _ctx: JobContext,
        payload: Self::Payload,
    ) -> Result<JobOutcome, HandlerError> {
        let n = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        if n < 2 {
            return Err(HandlerError::Other(format!(
                "first attempt for {}",
                payload.msg
            )));
        }
        Ok(JobOutcome::with_result(serde_json::json!({
            "attempt": n,
            "msg": payload.msg,
        })))
    }
}

#[tokio::test]
async fn worker_retries_failed_jobs_until_success() {
    let _guard = db_lock().await;
    let pool = fresh_pool().await;
    let repo = JobRepository::new(pool.clone());

    let payload = serde_json::to_value(FlakyPayload {
        msg: "retry-me".to_owned(),
    })
    .unwrap();
    let outcome = repo
        .enqueue(NewJob {
            kind: "test_flaky".to_owned(),
            payload,
            queue: None,
            priority: None,
            // Backoff would otherwise pin the second attempt 60s out; for
            // the test we keep max_attempts wide and rely on the lease
            // reaper / NOTIFY to wake the second attempt quickly. We
            // also manually clear available_at after the first failure
            // to skip the backoff window — that's a test-only knob and
            // matches what an operator-issued "retry now" would do.
            max_attempts: Some(5),
            idempotency_key: None,
            available_at: None,
            parent_job_id: None,
            correlation_id: None,
        })
        .await
        .expect("enqueue");
    let job_id = outcome.into_job().id;

    let attempts = Arc::new(AtomicUsize::new(0));
    let registry = JobRegistry::new().register(FlakyHandler {
        attempts: attempts.clone(),
    });

    // Background "fast-forward" task: as soon as we see the job back in
    // queued state with a future available_at, snap it forward so the
    // retry runs without waiting 60s of real time.
    let pool_for_advance = pool.clone();
    let advance_cancel = CancellationToken::new();
    let advance_cancel_for_task = advance_cancel.clone();
    let advance_handle = tokio::spawn(async move {
        loop {
            if advance_cancel_for_task.is_cancelled() {
                break;
            }
            let _ = sqlx::query(
                "UPDATE sbol_jobs SET available_at = now() \
                 WHERE id = $1 AND status = 'queued' AND available_at > now()",
            )
            .bind(job_id.as_uuid())
            .execute(&pool_for_advance)
            .await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    let succeeded = drive_worker_until(pool.clone(), registry, move |repo| {
        let id = job_id;
        async move {
            repo.current_status(id)
                .await
                .ok()
                .flatten()
                .map(|s| s == JobStatus::Succeeded)
                .unwrap_or(false)
        }
    })
    .await;
    advance_cancel.cancel();
    let _ = advance_handle.await;

    assert!(succeeded, "flaky job should eventually succeed after retry");
    assert!(
        attempts.load(Ordering::SeqCst) >= 2,
        "handler should have run at least twice (attempts={})",
        attempts.load(Ordering::SeqCst)
    );
}
