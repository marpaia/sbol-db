-- The async job queue: jobs plus their per-attempt audit trail and logs.
-- Timestamps are RFC3339 TEXT (sortable, all UTC); JSON payloads/results/fields
-- are TEXT. Lease-based dequeue is a single atomic UPDATE ... RETURNING under
-- SQLite's write lock (no SKIP LOCKED needed when the writer is serialized).

CREATE TABLE sbol_jobs (
    id               TEXT PRIMARY KEY,
    kind             TEXT NOT NULL,
    status           TEXT NOT NULL DEFAULT 'queued',
    priority         INTEGER NOT NULL DEFAULT 0,
    queue            TEXT NOT NULL DEFAULT 'default',
    payload          TEXT NOT NULL DEFAULT '{}',
    result           TEXT,
    error            TEXT,
    idempotency_key  TEXT,
    attempts         INTEGER NOT NULL DEFAULT 0,
    max_attempts     INTEGER NOT NULL DEFAULT 5,
    available_at     TEXT NOT NULL,
    leased_by        TEXT,
    lease_expires_at TEXT,
    parent_job_id    TEXT,
    correlation_id   TEXT,
    created_at       TEXT NOT NULL,
    started_at       TEXT,
    finished_at      TEXT
);

-- Dequeue scan: queued, ready, ordered by priority then age.
CREATE INDEX sbol_jobs_dequeue ON sbol_jobs (queue, status, priority, available_at);
CREATE INDEX sbol_jobs_status ON sbol_jobs (status);
CREATE INDEX sbol_jobs_correlation ON sbol_jobs (correlation_id);

CREATE TABLE sbol_job_attempts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id      TEXT NOT NULL REFERENCES sbol_jobs (id) ON DELETE CASCADE,
    attempt_no  INTEGER NOT NULL,
    worker_id   TEXT NOT NULL,
    started_at  TEXT NOT NULL,
    finished_at TEXT,
    status      TEXT NOT NULL,
    error       TEXT,
    UNIQUE (job_id, attempt_no)
);

CREATE TABLE sbol_job_logs (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id     TEXT NOT NULL REFERENCES sbol_jobs (id) ON DELETE CASCADE,
    attempt_no INTEGER,
    level      TEXT NOT NULL,
    message    TEXT NOT NULL,
    fields     TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL
);

CREATE INDEX sbol_job_logs_job ON sbol_job_logs (job_id, id);
