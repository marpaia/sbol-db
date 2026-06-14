-- Per-job log records for live operator feedback on job detail pages.
-- These are intentionally small, structured events rather than full
-- process logs; stdout/stderr logging remains the source for process-level
-- diagnostics.

CREATE TABLE sbol_job_logs (
    id          bigserial   PRIMARY KEY,
    job_id      uuid        NOT NULL REFERENCES sbol_jobs(id) ON DELETE CASCADE,
    attempt_no  integer,
    level       text        NOT NULL DEFAULT 'info'
        CHECK (level IN ('debug', 'info', 'warn', 'error')),
    message     text        NOT NULL,
    fields      jsonb       NOT NULL DEFAULT '{}'::jsonb,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX sbol_job_logs_job_idx
    ON sbol_job_logs (job_id, id);
