-- Async job queue for background and corpus-scale work distributed across
-- a cluster of sbol-db nodes. Postgres-backed (FOR UPDATE SKIP LOCKED) so
-- there is no sidecar broker; workers embed in `sbol-db serve` by default.

CREATE TYPE sbol_job_status AS ENUM (
    'queued',
    'running',
    'succeeded',
    'failed',
    'cancelled',
    'dead'
);

CREATE TABLE sbol_jobs (
    id                uuid           PRIMARY KEY DEFAULT gen_random_uuid(),
    kind              text           NOT NULL,
    status            sbol_job_status NOT NULL DEFAULT 'queued',
    priority          smallint       NOT NULL DEFAULT 0,
    queue             text           NOT NULL DEFAULT 'default',
    payload           jsonb          NOT NULL,
    result            jsonb,
    error             text,
    idempotency_key   text,
    attempts          integer        NOT NULL DEFAULT 0,
    max_attempts      integer        NOT NULL DEFAULT 5,
    available_at      timestamptz    NOT NULL DEFAULT now(),
    leased_by         text,
    lease_expires_at  timestamptz,
    parent_job_id     uuid           REFERENCES sbol_jobs(id) ON DELETE SET NULL,
    correlation_id    uuid,
    created_at        timestamptz    NOT NULL DEFAULT now(),
    started_at        timestamptz,
    finished_at       timestamptz
);

-- Dequeue path: ORDER BY priority DESC, available_at filtered to status='queued'
-- and a worker-supplied queue allowlist. The partial index keeps the hot
-- ordering tight even when the table accumulates terminal-state rows.
CREATE INDEX sbol_jobs_dequeue_idx
    ON sbol_jobs (queue, priority DESC, available_at)
    WHERE status = 'queued';

-- Reaper scans for status='running' rows whose lease has expired.
CREATE INDEX sbol_jobs_lease_expiry_idx
    ON sbol_jobs (lease_expires_at)
    WHERE status = 'running';

-- Idempotency dedup. Scoped to non-terminal + succeeded states so a failed
-- attempt with the same key can be retried after the prior row finalises.
CREATE UNIQUE INDEX sbol_jobs_idempotency_idx
    ON sbol_jobs (kind, idempotency_key)
    WHERE idempotency_key IS NOT NULL
      AND status IN ('queued', 'running', 'succeeded');

-- Operator triage by kind + status, ordered by recency.
CREATE INDEX sbol_jobs_kind_status_idx
    ON sbol_jobs (kind, status, created_at DESC);

-- Group fan-out / fan-in batches.
CREATE INDEX sbol_jobs_correlation_idx
    ON sbol_jobs (correlation_id)
    WHERE correlation_id IS NOT NULL;

CREATE TABLE sbol_job_attempts (
    id           bigserial   PRIMARY KEY,
    job_id       uuid        NOT NULL REFERENCES sbol_jobs(id) ON DELETE CASCADE,
    attempt_no   integer     NOT NULL,
    worker_id    text        NOT NULL,
    started_at   timestamptz NOT NULL DEFAULT now(),
    finished_at  timestamptz,
    status       sbol_job_status NOT NULL,
    error        text,
    UNIQUE (job_id, attempt_no)
);

CREATE INDEX sbol_job_attempts_job_idx
    ON sbol_job_attempts (job_id, attempt_no DESC);

-- LISTEN/NOTIFY: workers `LISTEN sbol_jobs_enqueued` and dequeue immediately
-- on insert rather than polling. The trigger fires only on INSERT or on
-- transitions back to 'queued' (retry path), not on every UPDATE.
CREATE OR REPLACE FUNCTION sbol_jobs_notify_enqueued() RETURNS trigger AS $$
BEGIN
    IF NEW.status = 'queued' THEN
        IF TG_OP = 'INSERT' OR OLD.status <> 'queued' THEN
            PERFORM pg_notify('sbol_jobs_enqueued', NEW.queue);
        END IF;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER sbol_jobs_notify_enqueued_trg
    AFTER INSERT OR UPDATE OF status ON sbol_jobs
    FOR EACH ROW EXECUTE FUNCTION sbol_jobs_notify_enqueued();
