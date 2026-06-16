//! Lifecycle tests for the job queue and worker. Run against the
//! docker-compose Postgres at `DATABASE_URL`. Each test serializes on a
//! process-wide mutex and TRUNCATEs the relevant tables, matching the
//! pattern in `worker_test.rs`.
//!
//! Tests use a dedicated `test_lifecycle_*` queue so that any background
//! `sbol-db serve` process (which polls the `default` queue) does not race
//! our enqueues. Each test picks a unique queue name.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use sbol_db_jobs::{
    HandlerError, JobContext, JobHandler, JobOutcome, JobRegistry, Worker, WorkerConfig,
};
use sbol_db_postgres::{
    connect, run_migrations, JobRepository, JobStatus, NewJob, SbolObjectService,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, MutexGuard};
use tokio_util::sync::CancellationToken;

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

fn new_job_on(queue: &str, kind: &str, payload: serde_json::Value) -> NewJob {
    NewJob {
        kind: kind.to_owned(),
        payload,
        queue: Some(queue.to_owned()),
        priority: None,
        max_attempts: None,
        idempotency_key: None,
        available_at: None,
        parent_job_id: None,
        correlation_id: None,
    }
}

// ---------------------------------------------------------------------------
// Direct JobRepository lifecycle tests (no worker — deterministic & fast)
// ---------------------------------------------------------------------------

/// Manually force a lease to expire, then assert `reap_expired_leases`
/// reclaims the job back to `queued` and a fresh dequeue picks it up.
#[tokio::test]
async fn reaper_reclaims_expired_lease() {
    let _guard = db_lock().await;
    let pool = fresh_pool().await;
    let repo = JobRepository::new(pool.clone());
    let queue = "test_lifecycle_reaper";

    let queued = repo
        .enqueue(new_job_on(
            queue,
            "test_lifecycle",
            serde_json::json!({"v": 1}),
        ))
        .await
        .expect("enqueue")
        .into_job();

    // Worker A grabs the lease.
    let leased = repo
        .dequeue(&[queue.to_owned()], "worker-a", Duration::from_secs(60))
        .await
        .expect("dequeue")
        .expect("got a job");
    assert_eq!(leased.id, queued.id);
    assert_eq!(leased.status, JobStatus::Running);
    assert_eq!(leased.leased_by.as_deref(), Some("worker-a"));

    // Force the lease to be expired in the past.
    sqlx::query(
        "UPDATE sbol_jobs SET lease_expires_at = now() - interval '1 second' WHERE id = $1",
    )
    .bind(leased.id.as_uuid())
    .execute(&pool)
    .await
    .expect("expire lease");

    let reaped = repo.reap_expired_leases().await.expect("reap");
    assert!(reaped >= 1, "at least one expired lease should be reaped");

    let status = repo
        .current_status(leased.id)
        .await
        .expect("status")
        .expect("present");
    assert_eq!(
        status,
        JobStatus::Queued,
        "reaped job must return to queued"
    );

    // A second worker can now claim it.
    let again = repo
        .dequeue(&[queue.to_owned()], "worker-b", Duration::from_secs(60))
        .await
        .expect("redequeue")
        .expect("present");
    assert_eq!(again.id, leased.id);
    assert_eq!(again.leased_by.as_deref(), Some("worker-b"));
    assert!(again.attempts >= 2, "redequeue should bump attempts");
}

/// Failing a job repeatedly transitions it through `queued -> running ->
/// queued` until the retry budget is exhausted, then to `dead`. The number
/// of attempts must be monotonically non-decreasing.
#[tokio::test]
async fn mark_failed_transitions_to_dead_after_budget() {
    let _guard = db_lock().await;
    let pool = fresh_pool().await;
    let repo = JobRepository::new(pool.clone());
    let queue = "test_lifecycle_dead";

    let job = repo
        .enqueue(NewJob {
            kind: "test_lifecycle".to_owned(),
            payload: serde_json::json!({}),
            queue: Some(queue.to_owned()),
            priority: None,
            max_attempts: Some(2),
            idempotency_key: None,
            available_at: None,
            parent_job_id: None,
            correlation_id: None,
        })
        .await
        .expect("enqueue")
        .into_job();
    assert_eq!(job.max_attempts, 2);

    let mut last_attempts = 0i32;
    let mut outcomes = Vec::new();
    for _ in 0..job.max_attempts {
        // Reset available_at so the backoff doesn't gate the test.
        sqlx::query("UPDATE sbol_jobs SET available_at = now() WHERE id = $1")
            .bind(job.id.as_uuid())
            .execute(&pool)
            .await
            .expect("reset available_at");

        let leased = repo
            .dequeue(&[queue.to_owned()], "worker-x", Duration::from_secs(60))
            .await
            .expect("dequeue")
            .expect("dequeue got job");
        assert!(
            leased.attempts > last_attempts,
            "attempts must increase: {} -> {}",
            last_attempts,
            leased.attempts
        );
        last_attempts = leased.attempts;
        let next = repo
            .mark_failed(job.id, "worker-x", "intentional")
            .await
            .expect("mark_failed");
        outcomes.push(next);
    }

    assert_eq!(
        outcomes.last(),
        Some(&JobStatus::Dead),
        "final transition must be Dead"
    );
    for s in &outcomes[..outcomes.len() - 1] {
        assert_eq!(*s, JobStatus::Queued, "non-final attempts re-queue");
    }

    let final_status = repo
        .current_status(job.id)
        .await
        .expect("status")
        .expect("present");
    assert_eq!(final_status, JobStatus::Dead);

    let attempts = repo.list_attempts(job.id).await.expect("list_attempts");
    assert_eq!(attempts.len(), job.max_attempts as usize);
    let mut nos: Vec<i32> = attempts.iter().map(|a| a.attempt_no).collect();
    nos.sort_unstable();
    assert_eq!(nos, (1..=job.max_attempts).collect::<Vec<_>>());
}

// ---------------------------------------------------------------------------
// Worker-integration tests
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CounterPayload {
    label: String,
}

struct PanickingHandler;

#[async_trait]
impl JobHandler for PanickingHandler {
    type Payload = CounterPayload;
    fn kind(&self) -> &'static str {
        "test_panic"
    }
    async fn run(
        &self,
        _ctx: JobContext,
        payload: Self::Payload,
    ) -> Result<JobOutcome, HandlerError> {
        panic!("intentional handler panic for {}", payload.label);
    }
}

struct CountingHandler {
    count: Arc<AtomicUsize>,
}

#[async_trait]
impl JobHandler for CountingHandler {
    type Payload = CounterPayload;
    fn kind(&self) -> &'static str {
        "test_counter"
    }
    async fn run(
        &self,
        _ctx: JobContext,
        payload: Self::Payload,
    ) -> Result<JobOutcome, HandlerError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(JobOutcome::with_result(serde_json::json!({
            "label": payload.label,
        })))
    }
}

struct SlowHandler {
    duration: Duration,
    started: Arc<AtomicUsize>,
    finished: Arc<AtomicUsize>,
}

#[async_trait]
impl JobHandler for SlowHandler {
    type Payload = CounterPayload;
    fn kind(&self) -> &'static str {
        "test_slow"
    }
    async fn run(
        &self,
        _ctx: JobContext,
        _payload: Self::Payload,
    ) -> Result<JobOutcome, HandlerError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.duration).await;
        self.finished.fetch_add(1, Ordering::SeqCst);
        Ok(JobOutcome::empty())
    }
}

fn fast_config(queue: &str) -> WorkerConfig {
    WorkerConfig {
        concurrency: 2,
        queues: vec![queue.to_owned()],
        lease: Duration::from_secs(10),
        renew_interval: Duration::from_secs(3),
        reaper_interval: Duration::from_millis(500),
        poll_interval: Duration::from_millis(100),
        shutdown_grace: Duration::from_secs(10),
        ..WorkerConfig::default()
    }
}

/// A handler that panics must not take the worker down. The worker should
/// continue to dequeue and run subsequent jobs.
#[tokio::test]
async fn worker_survives_panicking_handler() {
    let _guard = db_lock().await;
    let pool = fresh_pool().await;
    let repo = JobRepository::new(pool.clone());
    let queue = "test_lifecycle_panic";

    let panic_job = repo
        .enqueue(new_job_on(
            queue,
            "test_panic",
            serde_json::to_value(CounterPayload {
                label: "boom".into(),
            })
            .unwrap(),
        ))
        .await
        .expect("enqueue panic")
        .into_job();
    let ok_job = repo
        .enqueue(new_job_on(
            queue,
            "test_counter",
            serde_json::to_value(CounterPayload { label: "ok".into() }).unwrap(),
        ))
        .await
        .expect("enqueue ok")
        .into_job();

    let count = Arc::new(AtomicUsize::new(0));
    let registry = JobRegistry::new()
        .register(PanickingHandler)
        .register(CountingHandler {
            count: count.clone(),
        });
    let cancel = CancellationToken::new();
    let service = Arc::new(SbolObjectService::new(pool.clone()));
    let worker = Worker::new(
        pool.clone(),
        service,
        Arc::new(registry),
        fast_config(queue),
    );
    let cancel_clone = cancel.clone();
    let worker_handle = tokio::spawn(async move {
        worker.run(cancel_clone).await.expect("worker run");
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut ok_done = false;
    while tokio::time::Instant::now() < deadline {
        let s = repo
            .current_status(ok_job.id)
            .await
            .expect("status")
            .unwrap_or(JobStatus::Queued);
        if matches!(s, JobStatus::Succeeded) {
            ok_done = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    cancel.cancel();
    let _ = worker_handle.await;

    assert!(
        ok_done,
        "worker must keep processing jobs after a handler panic"
    );
    assert!(
        count.load(Ordering::SeqCst) >= 1,
        "counter handler must have run at least once"
    );

    let panic_status = repo
        .current_status(panic_job.id)
        .await
        .expect("status")
        .expect("present");
    assert_ne!(panic_status, JobStatus::Succeeded);
}

/// In-flight handlers must finish during the shutdown grace window.
#[tokio::test]
async fn graceful_shutdown_drains_in_flight_handler() {
    let _guard = db_lock().await;
    let pool = fresh_pool().await;
    let repo = JobRepository::new(pool.clone());
    let queue = "test_lifecycle_drain";

    let started = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicUsize::new(0));
    let slow_payload = serde_json::to_value(CounterPayload {
        label: "drain".into(),
    })
    .unwrap();
    let slow_job = repo
        .enqueue(new_job_on(queue, "test_slow", slow_payload))
        .await
        .expect("enqueue slow")
        .into_job();

    let registry = JobRegistry::new().register(SlowHandler {
        duration: Duration::from_millis(800),
        started: started.clone(),
        finished: finished.clone(),
    });
    let cancel = CancellationToken::new();
    let service = Arc::new(SbolObjectService::new(pool.clone()));
    let worker = Worker::new(
        pool.clone(),
        service,
        Arc::new(registry),
        fast_config(queue),
    );
    let cancel_clone = cancel.clone();
    let worker_handle = tokio::spawn(async move {
        worker.run(cancel_clone).await.expect("worker run");
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while started.load(Ordering::SeqCst) == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        started.load(Ordering::SeqCst),
        1,
        "slow handler should have started"
    );

    cancel.cancel();
    let _ = worker_handle.await;

    assert_eq!(
        finished.load(Ordering::SeqCst),
        1,
        "shutdown_grace should have let the in-flight handler finish"
    );
    let final_status = repo
        .current_status(slow_job.id)
        .await
        .expect("status")
        .expect("present");
    assert_eq!(
        final_status,
        JobStatus::Succeeded,
        "drained handler should have marked the job succeeded"
    );
}

/// Enqueueing a job whose `kind` has no registered handler must not crash
/// the worker; the worker must keep polling and pick up subsequent jobs.
#[tokio::test]
async fn unknown_kind_does_not_crash_worker() {
    let _guard = db_lock().await;
    let pool = fresh_pool().await;
    let repo = JobRepository::new(pool.clone());
    let queue = "test_lifecycle_unknown";

    let orphan = repo
        .enqueue(NewJob {
            kind: "no_handler_registered".to_owned(),
            payload: serde_json::json!({}),
            queue: Some(queue.to_owned()),
            priority: None,
            max_attempts: Some(1),
            idempotency_key: None,
            available_at: None,
            parent_job_id: None,
            correlation_id: None,
        })
        .await
        .expect("enqueue")
        .into_job();

    let count = Arc::new(AtomicUsize::new(0));
    let followup_payload = serde_json::to_value(CounterPayload {
        label: "follow".into(),
    })
    .unwrap();
    let followup = repo
        .enqueue(new_job_on(queue, "test_counter", followup_payload))
        .await
        .expect("enqueue followup")
        .into_job();

    let registry = JobRegistry::new().register(CountingHandler {
        count: count.clone(),
    });
    let cancel = CancellationToken::new();
    let service = Arc::new(SbolObjectService::new(pool.clone()));
    let worker = Worker::new(
        pool.clone(),
        service,
        Arc::new(registry),
        fast_config(queue),
    );
    let cancel_clone = cancel.clone();
    let worker_handle = tokio::spawn(async move {
        worker.run(cancel_clone).await.expect("worker run");
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut followup_done = false;
    while tokio::time::Instant::now() < deadline {
        let s = repo
            .current_status(followup.id)
            .await
            .expect("status")
            .unwrap_or(JobStatus::Queued);
        if matches!(s, JobStatus::Succeeded) {
            followup_done = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    cancel.cancel();
    let _ = worker_handle.await;

    assert!(
        followup_done,
        "worker should keep going after an unknown-kind job"
    );

    let orphan_status = repo
        .current_status(orphan.id)
        .await
        .expect("status")
        .expect("present");
    assert!(
        matches!(orphan_status, JobStatus::Dead | JobStatus::Failed),
        "orphan job should not succeed (got {orphan_status:?})"
    );
}
